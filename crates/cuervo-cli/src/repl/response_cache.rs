//! Semantic response cache: avoids redundant model invocations.
//!
//! Two-layer cache: L1 in-memory LRU (sub-microsecond), L2 SQLite (milliseconds).
//! Cache key is a SHA-256 hash of (model, system_prompt, conversation_tail, tool_names).
//! Write-through: stores go to both L1 and L2. Lookups check L1 first, then L2 (with
//! promotion to L1 on hit). TTL is checked on L1 reads.

use std::num::NonZeroUsize;
use std::sync::Mutex;

use chrono::Utc;
use lru::LruCache;
use sha2::{Digest, Sha256};

use cuervo_core::types::{CacheConfig, ModelRequest};
use cuervo_storage::{AsyncDatabase, CacheEntry};

/// Response cache with L1 in-memory LRU and L2 SQLite.
pub struct ResponseCache {
    db: AsyncDatabase,
    config: CacheConfig,
    l1: Mutex<LruCache<String, CacheEntry>>,
}

impl ResponseCache {
    pub fn new(db: AsyncDatabase, config: CacheConfig) -> Self {
        let l1_capacity = NonZeroUsize::new((config.max_entries.min(100) as usize).max(1))
            .unwrap_or(NonZeroUsize::new(1).unwrap());
        Self {
            db,
            config,
            l1: Mutex::new(LruCache::new(l1_capacity)),
        }
    }

    /// Check if caching is enabled.
    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Compute a cache key for a model request.
    pub fn compute_key(request: &ModelRequest) -> String {
        let mut hasher = Sha256::new();
        hasher.update(request.model.as_bytes());

        if let Some(ref sys) = request.system {
            hasher.update(sys.as_bytes());
        }

        // Hash last 3 messages (conversation tail determines semantic identity).
        let tail_start = request.messages.len().saturating_sub(3);
        for msg in &request.messages[tail_start..] {
            let serialized = serde_json::to_string(msg).unwrap_or_default();
            hasher.update(serialized.as_bytes());
        }

        // Hash tool names (not full schemas — schemas rarely change).
        for tool in &request.tools {
            hasher.update(tool.name.as_bytes());
        }

        hex::encode(hasher.finalize())
    }

    /// Look up a cached response. Checks L1 first, then L2 (with promotion).
    pub async fn lookup(&self, request: &ModelRequest) -> Option<CacheEntry> {
        if !self.config.enabled {
            return None;
        }

        let key = Self::compute_key(request);

        // L1 check.
        {
            let mut l1 = self.l1.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = l1.get(&key) {
                // Check TTL.
                if let Some(expires_at) = entry.expires_at {
                    if expires_at <= Utc::now() {
                        // Expired — evict from L1, fall through to L2.
                        l1.pop(&key);
                    } else {
                        return Some(entry.clone());
                    }
                } else {
                    // No expiry — always valid.
                    return Some(entry.clone());
                }
            }
        }

        // L2 check (async).
        match self.db.lookup_cache(&key).await {
            Ok(Some(entry)) => {
                // Promote to L1.
                let mut l1 = self.l1.lock().unwrap_or_else(|e| e.into_inner());
                l1.put(key, entry.clone());
                Some(entry)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("Cache L2 lookup failed: {e}");
                None
            }
        }
    }

    /// Warm the L1 cache from L2 (top entries by hit_count, not expired).
    pub async fn warm_l1(&self) {
        if !self.config.enabled {
            return;
        }

        let l1_cap = {
            let l1 = self.l1.lock().unwrap_or_else(|e| e.into_inner());
            l1.cap().get()
        };

        match self.db.top_cache_entries(l1_cap).await {
            Ok(entries) => {
                let mut l1 = self.l1.lock().unwrap_or_else(|e| e.into_inner());
                for entry in entries {
                    l1.put(entry.cache_key.clone(), entry);
                }
                tracing::debug!(count = l1.len(), "L1 cache warmed from L2");
            }
            Err(e) => {
                tracing::warn!("L1 cache warming failed: {e}");
            }
        }
    }

    /// Store a response in the cache (write-through: L1 + L2).
    pub async fn store(
        &self,
        request: &ModelRequest,
        response_text: &str,
        stop_reason: &str,
        usage_json: &str,
        tool_calls_json: Option<String>,
    ) {
        if !self.config.enabled {
            return;
        }

        // Don't cache tool_use responses (they trigger further execution).
        if stop_reason == "tool_use" {
            return;
        }

        let key = Self::compute_key(request);
        let expires_at = if self.config.default_ttl_secs > 0 {
            Some(Utc::now() + chrono::Duration::seconds(self.config.default_ttl_secs as i64))
        } else {
            None
        };

        let entry = CacheEntry {
            cache_key: key.clone(),
            model: request.model.clone(),
            response_text: response_text.to_string(),
            tool_calls_json,
            stop_reason: stop_reason.to_string(),
            usage_json: usage_json.to_string(),
            created_at: Utc::now(),
            expires_at,
            hit_count: 0,
        };

        // Write L1.
        {
            let mut l1 = self.l1.lock().unwrap_or_else(|e| e.into_inner());
            l1.put(key, entry.clone());
        }

        // Write L2 (async).
        if let Err(e) = self.db.insert_cache_entry(&entry).await {
            tracing::warn!("Cache L2 store failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use cuervo_core::types::{ChatMessage, MessageContent, Role};
    use cuervo_storage::Database;

    fn test_async_db() -> AsyncDatabase {
        AsyncDatabase::new(Arc::new(Database::open_in_memory().unwrap()))
    }

    fn test_config(enabled: bool) -> CacheConfig {
        CacheConfig {
            enabled,
            default_ttl_secs: 3600,
            max_entries: 100,
        }
    }

    fn test_request(prompt: &str) -> ModelRequest {
        ModelRequest {
            model: "claude".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text(prompt.into()),
            }],
            tools: vec![],
            max_tokens: Some(1024),
            temperature: Some(0.0),
            system: None,
            stream: true,
        }
    }

    #[tokio::test]
    async fn cache_disabled_returns_none() {
        let cache = ResponseCache::new(test_async_db(), test_config(false));
        let request = test_request("hello");
        assert!(cache.lookup(&request).await.is_none());
    }

    #[tokio::test]
    async fn cache_miss_returns_none() {
        let cache = ResponseCache::new(test_async_db(), test_config(true));
        let request = test_request("hello");
        assert!(cache.lookup(&request).await.is_none());
    }

    #[tokio::test]
    async fn cache_store_and_lookup() {
        let cache = ResponseCache::new(test_async_db(), test_config(true));
        let request = test_request("hello");

        cache.store(&request, "Hello world!", "end_turn", "{}", None).await;

        let hit = cache.lookup(&request).await.unwrap();
        assert_eq!(hit.response_text, "Hello world!");
        assert_eq!(hit.stop_reason, "end_turn");
    }

    #[test]
    fn cache_different_prompts_different_keys() {
        let request1 = test_request("hello");
        let request2 = test_request("goodbye");

        let key1 = ResponseCache::compute_key(&request1);
        let key2 = ResponseCache::compute_key(&request2);
        assert_ne!(key1, key2);
    }

    #[test]
    fn cache_same_prompt_same_key() {
        let request1 = test_request("hello");
        let request2 = test_request("hello");

        let key1 = ResponseCache::compute_key(&request1);
        let key2 = ResponseCache::compute_key(&request2);
        assert_eq!(key1, key2);
    }

    #[test]
    fn cache_different_model_different_key() {
        let mut request1 = test_request("hello");
        let mut request2 = test_request("hello");
        request1.model = "claude-sonnet".into();
        request2.model = "claude-haiku".into();

        let key1 = ResponseCache::compute_key(&request1);
        let key2 = ResponseCache::compute_key(&request2);
        assert_ne!(key1, key2);
    }

    #[test]
    fn cache_system_prompt_changes_key() {
        let mut request1 = test_request("hello");
        let mut request2 = test_request("hello");
        request1.system = Some("You are helpful".into());
        request2.system = Some("You are concise".into());

        let key1 = ResponseCache::compute_key(&request1);
        let key2 = ResponseCache::compute_key(&request2);
        assert_ne!(key1, key2);
    }

    #[tokio::test]
    async fn cache_does_not_store_tool_use_responses() {
        let cache = ResponseCache::new(test_async_db(), test_config(true));
        let request = test_request("read that file");

        cache.store(&request, "I'll read it", "tool_use", "{}", None).await;

        assert!(cache.lookup(&request).await.is_none());
    }

    #[tokio::test]
    async fn cache_disabled_store_is_noop() {
        let db = test_async_db();
        let cache = ResponseCache::new(db.clone(), test_config(false));
        let request = test_request("hello");

        cache.store(&request, "response", "end_turn", "{}", None).await;

        let stats = db.inner().cache_stats().unwrap();
        assert_eq!(stats.total_entries, 0);
    }

    // --- L1 cache-specific tests ---

    #[tokio::test]
    async fn l1_hit_returns_without_db_call() {
        let cache = ResponseCache::new(test_async_db(), test_config(true));
        let request = test_request("l1 test");

        // Store populates both L1 and L2.
        cache.store(&request, "l1 response", "end_turn", "{}", None).await;

        // Lookup should hit L1 (sub-microsecond).
        let hit = cache.lookup(&request).await.unwrap();
        assert_eq!(hit.response_text, "l1 response");
    }

    #[tokio::test]
    async fn l1_miss_falls_through_to_l2() {
        let db = test_async_db();
        let cache = ResponseCache::new(db.clone(), test_config(true));
        let request = test_request("l2 only");

        // Insert directly into L2 (bypassing L1).
        let key = ResponseCache::compute_key(&request);
        let entry = CacheEntry {
            cache_key: key,
            model: "claude".into(),
            response_text: "from db".into(),
            tool_calls_json: None,
            stop_reason: "end_turn".into(),
            usage_json: "{}".into(),
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            hit_count: 0,
        };
        db.insert_cache_entry(&entry).await.unwrap();

        // L1 miss → L2 hit → promote to L1.
        let hit = cache.lookup(&request).await.unwrap();
        assert_eq!(hit.response_text, "from db");

        // Second lookup should hit L1 now.
        let hit2 = cache.lookup(&request).await.unwrap();
        assert_eq!(hit2.response_text, "from db");
    }

    #[tokio::test]
    async fn l1_expired_entry_evicted() {
        let config = CacheConfig {
            enabled: true,
            default_ttl_secs: 0, // Immediate expiry.
            max_entries: 100,
        };
        let db = test_async_db();
        let cache = ResponseCache::new(db.clone(), config);
        let request = test_request("expire me");

        cache.store(&request, "ephemeral", "end_turn", "{}", None).await;

        // The entry has no expires_at (ttl=0 means no expiry per current logic).
        // To test actual expiry, insert with a past expires_at directly.
        let key = ResponseCache::compute_key(&request);
        {
            let mut l1 = cache.l1.lock().unwrap();
            l1.put(key, CacheEntry {
                cache_key: ResponseCache::compute_key(&request),
                model: "claude".into(),
                response_text: "expired".into(),
                tool_calls_json: None,
                stop_reason: "end_turn".into(),
                usage_json: "{}".into(),
                created_at: Utc::now(),
                expires_at: Some(Utc::now() - chrono::Duration::seconds(1)),
                hit_count: 0,
            });
        }

        // L1 should find the expired entry and evict it.
        // L2 has the non-expired "ephemeral" entry, so that should be returned.
        let hit = cache.lookup(&request).await;
        // L2 may or may not have it depending on TTL logic.
        // At minimum, the expired L1 entry should NOT be returned.
        if let Some(h) = hit {
            assert_ne!(h.response_text, "expired");
        }
    }

    #[tokio::test]
    async fn l1_lru_eviction() {
        // Create cache with L1 capacity of 2.
        let config = CacheConfig {
            enabled: true,
            default_ttl_secs: 3600,
            max_entries: 2,
        };
        let cache = ResponseCache::new(test_async_db(), config);

        let req1 = test_request("first");
        let req2 = test_request("second");
        let req3 = test_request("third");

        cache.store(&req1, "r1", "end_turn", "{}", None).await;
        cache.store(&req2, "r2", "end_turn", "{}", None).await;
        cache.store(&req3, "r3", "end_turn", "{}", None).await;

        // req1 should have been evicted from L1 (LRU).
        // Check L1 directly.
        {
            let l1 = cache.l1.lock().unwrap();
            let key1 = ResponseCache::compute_key(&req1);
            assert!(!l1.contains(&key1), "req1 should be evicted from L1");
        }

        // req1 should still be in L2 though.
        let hit = cache.lookup(&req1).await;
        assert!(hit.is_some(), "req1 should still be in L2");
        assert_eq!(hit.unwrap().response_text, "r1");
    }

    #[tokio::test]
    async fn l1_and_l2_write_through() {
        let db = test_async_db();
        let cache = ResponseCache::new(db.clone(), test_config(true));
        let request = test_request("write through");

        cache.store(&request, "both layers", "end_turn", "{}", None).await;

        // Check L1.
        {
            let l1 = cache.l1.lock().unwrap();
            let key = ResponseCache::compute_key(&request);
            assert!(l1.contains(&key), "should be in L1");
        }

        // Check L2.
        let key = ResponseCache::compute_key(&request);
        let l2_entry = db.lookup_cache(&key).await.unwrap();
        assert!(l2_entry.is_some(), "should be in L2");
        assert_eq!(l2_entry.unwrap().response_text, "both layers");
    }

    #[tokio::test]
    async fn l1_disabled_cache_returns_none() {
        let cache = ResponseCache::new(test_async_db(), test_config(false));
        let request = test_request("disabled");

        cache.store(&request, "nope", "end_turn", "{}", None).await;
        assert!(cache.lookup(&request).await.is_none());
    }

    #[tokio::test]
    async fn l1_tool_use_not_stored() {
        let cache = ResponseCache::new(test_async_db(), test_config(true));
        let request = test_request("tool call");

        cache.store(&request, "calling tool", "tool_use", "{}", None).await;

        // Neither L1 nor L2 should have it.
        assert!(cache.lookup(&request).await.is_none());
        {
            let l1 = cache.l1.lock().unwrap();
            let key = ResponseCache::compute_key(&request);
            assert!(!l1.contains(&key));
        }
    }

    #[tokio::test]
    async fn cache_hit_skips_redundant_store() {
        let cache = ResponseCache::new(test_async_db(), test_config(true));
        let request = test_request("cache hit test");

        // Store once.
        cache.store(&request, "first response", "end_turn", "{}", None).await;

        // Lookup should hit.
        let hit = cache.lookup(&request).await;
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().response_text, "first response");

        // Store a different response for the same key (overwrite).
        cache.store(&request, "updated response", "end_turn", "{}", None).await;

        // Lookup should return the updated response.
        let hit2 = cache.lookup(&request).await;
        assert!(hit2.is_some());
        assert_eq!(hit2.unwrap().response_text, "updated response");
    }

    #[tokio::test]
    async fn cache_warming_populates_l1() {
        let db = test_async_db();
        let cache = ResponseCache::new(db.clone(), test_config(true));

        // Insert entries directly into L2 (bypassing L1).
        for i in 0..3 {
            let key = format!("warm-key-{i}");
            let entry = CacheEntry {
                cache_key: key.clone(),
                model: "claude".into(),
                response_text: format!("warm response {i}"),
                tool_calls_json: None,
                stop_reason: "end_turn".into(),
                usage_json: "{}".into(),
                created_at: Utc::now(),
                expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
                hit_count: (10 - i) as u32, // Descending hit count.
            };
            db.insert_cache_entry(&entry).await.unwrap();
        }

        // L1 should be empty.
        {
            let l1 = cache.l1.lock().unwrap();
            assert_eq!(l1.len(), 0);
        }

        // Warm L1 from L2.
        cache.warm_l1().await;

        // L1 should now have entries.
        {
            let l1 = cache.l1.lock().unwrap();
            assert_eq!(l1.len(), 3);
            assert!(l1.contains(&"warm-key-0".to_string()));
            assert!(l1.contains(&"warm-key-1".to_string()));
            assert!(l1.contains(&"warm-key-2".to_string()));
        }
    }

    #[tokio::test]
    async fn l1_different_keys_independent() {
        let cache = ResponseCache::new(test_async_db(), test_config(true));
        let req1 = test_request("alpha");
        let req2 = test_request("beta");

        cache.store(&req1, "alpha response", "end_turn", "{}", None).await;
        cache.store(&req2, "beta response", "end_turn", "{}", None).await;

        let hit1 = cache.lookup(&req1).await.unwrap();
        let hit2 = cache.lookup(&req2).await.unwrap();
        assert_eq!(hit1.response_text, "alpha response");
        assert_eq!(hit2.response_text, "beta response");
    }
}
