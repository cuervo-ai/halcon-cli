//! Performance measurement tests for the cuervo-storage layer.
//!
//! Run with: cargo test -p cuervo-storage --test perf_measurements -- --nocapture
//!
//! These are NOT assertions — they print timing results for each operation
//! so you can monitor storage performance characteristics over time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use cuervo_storage::{
    AsyncDatabase, CacheEntry, Database, InvocationMetric, MemoryEntry, MemoryEntryType,
    MemoryEpisode,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_memory_entry(i: usize, entry_type: MemoryEntryType) -> MemoryEntry {
    let content = format!(
        "Memory entry number {} with unique content for testing storage throughput and search. \
         Keywords: rust, async, tokio, sqlite, benchmark, performance, cuervo, cli, tool, agent, \
         index-{i}",
        i
    );
    let hash = hex::encode(Sha256::digest(content.as_bytes()));
    MemoryEntry {
        entry_id: Uuid::new_v4(),
        session_id: None,
        entry_type,
        content,
        content_hash: hash,
        metadata: serde_json::json!({"index": i}),
        created_at: Utc::now(),
        expires_at: None,
        relevance_score: 1.0,
    }
}

fn make_cache_entry(i: usize) -> CacheEntry {
    CacheEntry {
        cache_key: format!("cache-key-{i}"),
        model: "claude-sonnet-4-5".into(),
        response_text: format!("Response text for cache entry {i} with some padding to simulate realistic response sizes. Lorem ipsum dolor sit amet."),
        tool_calls_json: None,
        stop_reason: "end_turn".into(),
        usage_json: r#"{"input_tokens":100,"output_tokens":50}"#.into(),
        created_at: Utc::now(),
        expires_at: None,
        hit_count: 0,
    }
}

fn make_metric(i: usize, provider: &str, model: &str) -> InvocationMetric {
    InvocationMetric {
        provider: provider.to_string(),
        model: model.to_string(),
        latency_ms: 100 + (i as u64 % 900), // 100-999ms range
        input_tokens: 100 + (i as u32 % 500),
        output_tokens: 50 + (i as u32 % 200),
        estimated_cost_usd: 0.001 * (i as f64 % 10.0),
        success: i % 10 != 0, // 90% success rate
        stop_reason: if i % 10 == 0 {
            "error".into()
        } else {
            "end_turn".into()
        },
        session_id: None,
        created_at: Utc::now(),
    }
}

fn percentile(sorted: &[Duration], pct: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() as f64 * pct / 100.0).ceil() as usize).min(sorted.len()) - 1;
    sorted[idx]
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_micros();
    if us < 1000 {
        format!("{us}us")
    } else {
        format!("{:.1}ms", us as f64 / 1000.0)
    }
}

fn fmt_ops(count: usize, elapsed: Duration) -> String {
    let ops = count as f64 / elapsed.as_secs_f64();
    if ops >= 1000.0 {
        format!("{:.0} ops/sec", ops)
    } else {
        format!("{:.1} ops/sec", ops)
    }
}

// ---------------------------------------------------------------------------
// 1. Memory insertion throughput
// ---------------------------------------------------------------------------

#[test]
fn bench_memory_insertion_throughput() {
    let db = Database::open_in_memory().unwrap();

    for &count in &[1000usize, 5000, 10000] {
        // Use a fresh DB for each tier to avoid duplicate hash rejection
        let db = Database::open_in_memory().unwrap();
        let start = Instant::now();
        for i in 0..count {
            let entry_type = match i % 5 {
                0 => MemoryEntryType::Fact,
                1 => MemoryEntryType::SessionSummary,
                2 => MemoryEntryType::Decision,
                3 => MemoryEntryType::CodeSnippet,
                _ => MemoryEntryType::ProjectMeta,
            };
            let entry = make_memory_entry(i, entry_type);
            db.insert_memory(&entry).unwrap();
        }
        let elapsed = start.elapsed();
        println!(
            "[BENCH] insert_{count}_memories: {} ({})",
            fmt_dur(elapsed),
            fmt_ops(count, elapsed)
        );
    }

    // Keep the outer db alive to silence unused warning
    let _ = db;
}

// ---------------------------------------------------------------------------
// 2. FTS5 search latency
// ---------------------------------------------------------------------------

#[test]
fn bench_fts5_search_latency() {
    let db = Database::open_in_memory().unwrap();

    // Insert 10000 entries
    for i in 0..10000 {
        let entry_type = match i % 5 {
            0 => MemoryEntryType::Fact,
            1 => MemoryEntryType::SessionSummary,
            2 => MemoryEntryType::Decision,
            3 => MemoryEntryType::CodeSnippet,
            _ => MemoryEntryType::ProjectMeta,
        };
        let entry = make_memory_entry(i, entry_type);
        db.insert_memory(&entry).unwrap();
    }

    let queries = [
        "rust async", "sqlite benchmark", "performance tool",
        "agent cli", "tokio cuervo", "unique content", "memory entry",
        "storage throughput", "search index", "number testing",
    ];

    let mut latencies = Vec::with_capacity(100);
    for i in 0..100 {
        let q = queries[i % queries.len()];
        let start = Instant::now();
        let results = db.search_memory_fts(q, 10).unwrap();
        let elapsed = start.elapsed();
        latencies.push(elapsed);
        // Verify we get results
        assert!(!results.is_empty(), "FTS query '{q}' returned no results");
    }

    latencies.sort();
    println!(
        "[BENCH] fts5_search_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&latencies, 50.0)),
        fmt_dur(percentile(&latencies, 95.0)),
        fmt_dur(percentile(&latencies, 99.0)),
    );
}

// ---------------------------------------------------------------------------
// 3. Embedding storage + cosine search
// ---------------------------------------------------------------------------

#[test]
fn bench_embedding_storage_and_search() {
    let db = Database::open_in_memory().unwrap();
    let dim = 384;

    // Insert 1000 entries with embeddings
    let mut entry_ids = Vec::with_capacity(1000);
    for i in 0..1000 {
        let entry = make_memory_entry(i, MemoryEntryType::Fact);
        let uuid_str = entry.entry_id.to_string();
        entry_ids.push(uuid_str.clone());
        db.insert_memory(&entry).unwrap();
    }

    // Store embeddings
    let embed_start = Instant::now();
    for (i, uuid_str) in entry_ids.iter().enumerate() {
        // Generate a deterministic embedding: normalized vector
        let embedding: Vec<f32> = (0..dim)
            .map(|d| ((i * 31 + d * 7) as f32 % 100.0) / 100.0)
            .collect();
        db.update_entry_embedding(uuid_str, &embedding, "test-model")
            .unwrap();
    }
    let embed_elapsed = embed_start.elapsed();
    println!(
        "[BENCH] store_1000_embeddings: {} ({})",
        fmt_dur(embed_elapsed),
        fmt_ops(1000, embed_elapsed)
    );

    // Search by embedding 100 times
    let mut latencies = Vec::with_capacity(100);
    for i in 0..100 {
        let query_vec: Vec<f32> = (0..dim)
            .map(|d| ((i * 13 + d * 3) as f32 % 100.0) / 100.0)
            .collect();
        let start = Instant::now();
        let results = db.search_memory_by_embedding(&query_vec, 10).unwrap();
        let elapsed = start.elapsed();
        latencies.push(elapsed);
        assert!(!results.is_empty(), "Embedding search returned no results");
    }

    latencies.sort();
    println!(
        "[BENCH] embedding_search_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&latencies, 50.0)),
        fmt_dur(percentile(&latencies, 95.0)),
        fmt_dur(percentile(&latencies, 99.0)),
    );
}

// ---------------------------------------------------------------------------
// 4. Episode CRUD
// ---------------------------------------------------------------------------

#[test]
fn bench_episode_crud() {
    let db = Database::open_in_memory().unwrap();

    // First insert 1000 memory entries (we'll link 10 per episode)
    let mut entry_ids = Vec::with_capacity(1000);
    for i in 0..1000 {
        let entry = make_memory_entry(i, MemoryEntryType::Fact);
        entry_ids.push(entry.entry_id.to_string());
        db.insert_memory(&entry).unwrap();
    }

    // Create 100 episodes
    let mut episode_ids = Vec::with_capacity(100);
    let mut save_latencies = Vec::with_capacity(100);
    for i in 0..100 {
        let ep_id = format!("episode-{i}");
        episode_ids.push(ep_id.clone());
        let episode = MemoryEpisode {
            episode_id: ep_id,
            session_id: None,
            title: format!("Episode {i}: Testing storage performance"),
            summary: Some(format!("Summary of episode {i}")),
            started_at: Utc::now(),
            ended_at: None,
            metadata: serde_json::json!({"episode_index": i}),
        };
        let start = Instant::now();
        db.save_episode(&episode).unwrap();
        save_latencies.push(start.elapsed());
    }
    save_latencies.sort();
    println!(
        "[BENCH] save_episode_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&save_latencies, 50.0)),
        fmt_dur(percentile(&save_latencies, 95.0)),
        fmt_dur(percentile(&save_latencies, 99.0)),
    );

    // Link 10 entries to each episode
    let mut link_latencies = Vec::with_capacity(1000);
    for (ep_idx, ep_id) in episode_ids.iter().enumerate() {
        for pos in 0..10u32 {
            let entry_uuid = &entry_ids[ep_idx * 10 + pos as usize];
            let start = Instant::now();
            db.link_entry_to_episode(entry_uuid, ep_id, pos).unwrap();
            link_latencies.push(start.elapsed());
        }
    }
    link_latencies.sort();
    println!(
        "[BENCH] link_entry_to_episode_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&link_latencies, 50.0)),
        fmt_dur(percentile(&link_latencies, 95.0)),
        fmt_dur(percentile(&link_latencies, 99.0)),
    );

    // Load entries for each episode
    let mut load_latencies = Vec::with_capacity(100);
    for ep_id in &episode_ids {
        let start = Instant::now();
        let entries = db.load_episode_entries(ep_id).unwrap();
        let elapsed = start.elapsed();
        load_latencies.push(elapsed);
        assert_eq!(entries.len(), 10, "Expected 10 entries per episode");
    }
    load_latencies.sort();
    println!(
        "[BENCH] load_episode_entries_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&load_latencies, 50.0)),
        fmt_dur(percentile(&load_latencies, 95.0)),
        fmt_dur(percentile(&load_latencies, 99.0)),
    );
}

// ---------------------------------------------------------------------------
// 5. Session save/load
// ---------------------------------------------------------------------------

#[test]
fn bench_session_save_load() {
    use cuervo_core::types::{ChatMessage, MessageContent, Role, Session, TokenUsage};

    let db = Database::open_in_memory().unwrap();

    for &msg_count in &[0usize, 100, 500, 1000] {
        let mut session = Session::new("claude-sonnet-4-5".into(), "anthropic".into(), "/tmp".into());
        for i in 0..msg_count {
            session.messages.push(ChatMessage {
                role: if i % 2 == 0 { Role::User } else { Role::Assistant },
                content: MessageContent::Text(format!(
                    "Message {i} with realistic content: This is a test message for benchmarking. \
                     It includes enough text to simulate a real conversation turn with the model."
                )),
            });
        }
        session.total_usage = TokenUsage {
            input_tokens: msg_count as u32 * 50,
            output_tokens: msg_count as u32 * 100,
            ..Default::default()
        };
        let id = session.id;

        let save_start = Instant::now();
        db.save_session(&session).unwrap();
        let save_elapsed = save_start.elapsed();

        let load_start = Instant::now();
        let loaded = db.load_session(id).unwrap().unwrap();
        let load_elapsed = load_start.elapsed();

        assert_eq!(loaded.messages.len(), msg_count);

        println!(
            "[BENCH] session_save_{msg_count}_msgs: {}, session_load_{msg_count}_msgs: {}",
            fmt_dur(save_elapsed),
            fmt_dur(load_elapsed),
        );
    }
}

// ---------------------------------------------------------------------------
// 6. Cache operations
// ---------------------------------------------------------------------------

#[test]
fn bench_cache_operations() {
    let db = Database::open_in_memory().unwrap();

    // Insert 1000 cache entries
    let insert_start = Instant::now();
    for i in 0..1000 {
        let entry = make_cache_entry(i);
        db.insert_cache_entry(&entry).unwrap();
    }
    let insert_elapsed = insert_start.elapsed();
    println!(
        "[BENCH] cache_insert_1000: {} ({})",
        fmt_dur(insert_elapsed),
        fmt_ops(1000, insert_elapsed)
    );

    // Lookup 1000 times: 500 hits + 500 misses
    let mut hit_latencies = Vec::with_capacity(500);
    let mut miss_latencies = Vec::with_capacity(500);
    for i in 0..1000 {
        if i % 2 == 0 {
            // Hit
            let key = format!("cache-key-{}", i / 2);
            let start = Instant::now();
            let result = db.lookup_cache(&key).unwrap();
            hit_latencies.push(start.elapsed());
            assert!(result.is_some(), "Expected cache hit for {key}");
        } else {
            // Miss
            let key = format!("nonexistent-key-{i}");
            let start = Instant::now();
            let result = db.lookup_cache(&key).unwrap();
            miss_latencies.push(start.elapsed());
            assert!(result.is_none(), "Expected cache miss for {key}");
        }
    }

    hit_latencies.sort();
    miss_latencies.sort();

    let total_lookup = Instant::now();
    for i in 0..1000 {
        let key = format!("cache-key-{}", i % 1000);
        let _ = db.lookup_cache(&key).unwrap();
    }
    let lookup_elapsed = total_lookup.elapsed();

    println!(
        "[BENCH] cache_hit_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&hit_latencies, 50.0)),
        fmt_dur(percentile(&hit_latencies, 95.0)),
        fmt_dur(percentile(&hit_latencies, 99.0)),
    );
    println!(
        "[BENCH] cache_miss_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&miss_latencies, 50.0)),
        fmt_dur(percentile(&miss_latencies, 95.0)),
        fmt_dur(percentile(&miss_latencies, 99.0)),
    );
    println!(
        "[BENCH] cache_lookup_1000_total: {} ({})",
        fmt_dur(lookup_elapsed),
        fmt_ops(1000, lookup_elapsed)
    );
}

// ---------------------------------------------------------------------------
// 7. Concurrent access (tokio tasks)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bench_concurrent_access() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let adb = AsyncDatabase::new(db.clone());

    let ops_per_task = 250;
    let num_tasks = 4;
    let total_ops = ops_per_task * num_tasks;

    // --- Single-threaded baseline ---
    let single_start = Instant::now();
    for i in 0..total_ops {
        let entry = make_memory_entry(i + 100_000, MemoryEntryType::Fact);
        adb.insert_memory(&entry).await.unwrap();
    }
    let single_elapsed = single_start.elapsed();

    // Clean up for concurrent test (use separate offset range)
    let adb_clone = adb.clone();

    // --- Concurrent (4 tasks) ---
    let concurrent_start = Instant::now();
    let mut handles = Vec::with_capacity(num_tasks);
    for task_id in 0..num_tasks {
        let adb_inner = adb_clone.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..ops_per_task {
                let global_idx = 200_000 + task_id * ops_per_task + i;
                let entry = make_memory_entry(global_idx, MemoryEntryType::Decision);
                adb_inner.insert_memory(&entry).await.unwrap();
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let concurrent_elapsed = concurrent_start.elapsed();

    // --- Concurrent mixed: 2 writers + 2 readers ---
    let mixed_start = Instant::now();
    let mut handles2 = Vec::with_capacity(4);
    // 2 writer tasks
    for task_id in 0..2 {
        let adb_inner = adb_clone.clone();
        handles2.push(tokio::spawn(async move {
            for i in 0..ops_per_task {
                let global_idx = 300_000 + task_id * ops_per_task + i;
                let entry = make_memory_entry(global_idx, MemoryEntryType::CodeSnippet);
                adb_inner.insert_memory(&entry).await.unwrap();
            }
        }));
    }
    // 2 reader tasks
    for _ in 0..2 {
        let adb_inner = adb_clone.clone();
        handles2.push(tokio::spawn(async move {
            for _ in 0..ops_per_task {
                adb_inner.list_memories(None, 10).await.unwrap();
            }
        }));
    }
    for h in handles2 {
        h.await.unwrap();
    }
    let mixed_elapsed = mixed_start.elapsed();

    println!(
        "[BENCH] single_thread_{total_ops}_inserts: {} ({})",
        fmt_dur(single_elapsed),
        fmt_ops(total_ops, single_elapsed)
    );
    println!(
        "[BENCH] concurrent_4_tasks_{total_ops}_inserts: {} ({})",
        fmt_dur(concurrent_elapsed),
        fmt_ops(total_ops, concurrent_elapsed)
    );
    println!(
        "[BENCH] concurrent_mixed_2w2r_{}_ops: {} ({})",
        ops_per_task * 4,
        fmt_dur(mixed_elapsed),
        fmt_ops(ops_per_task * 4, mixed_elapsed)
    );
    let speedup = single_elapsed.as_secs_f64() / concurrent_elapsed.as_secs_f64();
    println!(
        "[BENCH] concurrency_speedup_ratio: {:.2}x (>1.0 = concurrent is faster)",
        speedup
    );
}

// ---------------------------------------------------------------------------
// 8. DB growth estimation
// ---------------------------------------------------------------------------

#[test]
fn bench_db_growth() {
    let db = Database::open_in_memory().unwrap();

    // Insert 10000 memory entries
    for i in 0..10000 {
        let entry_type = match i % 5 {
            0 => MemoryEntryType::Fact,
            1 => MemoryEntryType::SessionSummary,
            2 => MemoryEntryType::Decision,
            3 => MemoryEntryType::CodeSnippet,
            _ => MemoryEntryType::ProjectMeta,
        };
        let entry = make_memory_entry(i, entry_type);
        db.insert_memory(&entry).unwrap();
    }

    // Insert 1000 cache entries
    for i in 0..1000 {
        let entry = make_cache_entry(i);
        db.insert_cache_entry(&entry).unwrap();
    }

    // Insert 100 sessions with 10 messages each
    use cuervo_core::types::{ChatMessage, MessageContent, Role, Session};
    for i in 0..100 {
        let mut session =
            Session::new("claude-sonnet-4-5".into(), "anthropic".into(), "/tmp".into());
        for j in 0..10 {
            session.messages.push(ChatMessage {
                role: if j % 2 == 0 { Role::User } else { Role::Assistant },
                content: MessageContent::Text(format!("Session {i} message {j}")),
            });
        }
        db.save_session(&session).unwrap();
    }

    // Insert 5000 metrics
    let providers = ["anthropic", "ollama", "openai"];
    let models = [
        "claude-sonnet-4-5",
        "llama-3-70b",
        "gpt-4",
    ];
    for i in 0..5000 {
        let p = providers[i % 3];
        let m = models[i % 3];
        let metric = make_metric(i, p, m);
        db.insert_metric(&metric).unwrap();
    }

    // Report memory stats
    let mem_stats = db.memory_stats().unwrap();
    let cache_stats = db.cache_stats().unwrap();

    println!("[BENCH] db_growth_summary:");
    println!("  memory_entries: {}", mem_stats.total_entries);
    println!("  cache_entries: {}", cache_stats.total_entries);
    println!("  sessions: 100");
    println!("  metrics: 5000");
    println!(
        "  memory_by_type: {:?}",
        mem_stats.by_type
    );

    // Estimate: get page_count * page_size via PRAGMA
    // Note: in-memory DB doesn't have a file, but we can still query page info
    // We print what we can.
    println!("  (in-memory DB -- no file size; data volume is proportional to entry counts)");
}

// ---------------------------------------------------------------------------
// 9. Metric aggregation
// ---------------------------------------------------------------------------

#[test]
fn bench_metric_aggregation() {
    let db = Database::open_in_memory().unwrap();

    let providers = ["anthropic", "ollama", "openai"];
    let models = [
        "claude-sonnet-4-5",
        "llama-3-70b",
        "gpt-4",
    ];

    // Insert 5000 metrics across 3 providers
    let insert_start = Instant::now();
    for i in 0..5000 {
        let p = providers[i % 3];
        let m = models[i % 3];
        let metric = make_metric(i, p, m);
        db.insert_metric(&metric).unwrap();
    }
    let insert_elapsed = insert_start.elapsed();
    println!(
        "[BENCH] insert_5000_metrics: {} ({})",
        fmt_dur(insert_elapsed),
        fmt_ops(5000, insert_elapsed)
    );

    // Measure system_metrics()
    let mut sys_latencies = Vec::with_capacity(20);
    for _ in 0..20 {
        let start = Instant::now();
        let sys = db.system_metrics().unwrap();
        sys_latencies.push(start.elapsed());
        assert_eq!(sys.total_invocations, 5000);
        assert_eq!(sys.models.len(), 3);
    }
    sys_latencies.sort();
    println!(
        "[BENCH] system_metrics_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&sys_latencies, 50.0)),
        fmt_dur(percentile(&sys_latencies, 95.0)),
        fmt_dur(percentile(&sys_latencies, 99.0)),
    );

    // Measure model_stats() per model
    let mut model_latencies = Vec::with_capacity(60);
    for _ in 0..20 {
        for j in 0..3 {
            let start = Instant::now();
            let stats = db.model_stats(providers[j], models[j]).unwrap();
            model_latencies.push(start.elapsed());
            assert!(stats.total_invocations > 0);
        }
    }
    model_latencies.sort();
    println!(
        "[BENCH] model_stats_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&model_latencies, 50.0)),
        fmt_dur(percentile(&model_latencies, 95.0)),
        fmt_dur(percentile(&model_latencies, 99.0)),
    );

    // Measure provider_metrics_windowed()
    let mut windowed_latencies = Vec::with_capacity(60);
    for _ in 0..20 {
        for p in &providers {
            let start = Instant::now();
            let _wm = db.provider_metrics_windowed(p, 60).unwrap();
            windowed_latencies.push(start.elapsed());
        }
    }
    windowed_latencies.sort();
    println!(
        "[BENCH] provider_windowed_metrics_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&windowed_latencies, 50.0)),
        fmt_dur(percentile(&windowed_latencies, 95.0)),
        fmt_dur(percentile(&windowed_latencies, 99.0)),
    );
}

// ---------------------------------------------------------------------------
// 10. Memory list with type filter
// ---------------------------------------------------------------------------

#[test]
fn bench_memory_list_with_type_filter() {
    let db = Database::open_in_memory().unwrap();

    // Insert 10000 entries of mixed types
    for i in 0..10000 {
        let entry_type = match i % 5 {
            0 => MemoryEntryType::Fact,
            1 => MemoryEntryType::SessionSummary,
            2 => MemoryEntryType::Decision,
            3 => MemoryEntryType::CodeSnippet,
            _ => MemoryEntryType::ProjectMeta,
        };
        let entry = make_memory_entry(i, entry_type);
        db.insert_memory(&entry).unwrap();
    }

    // Measure list_memories(Some(Fact), 100)
    let mut filtered_latencies = Vec::with_capacity(50);
    for _ in 0..50 {
        let start = Instant::now();
        let entries = db.list_memories(Some(MemoryEntryType::Fact), 100).unwrap();
        filtered_latencies.push(start.elapsed());
        assert_eq!(entries.len(), 100);
    }
    filtered_latencies.sort();

    // Measure list_memories(None, 100)
    let mut unfiltered_latencies = Vec::with_capacity(50);
    for _ in 0..50 {
        let start = Instant::now();
        let entries = db.list_memories(None, 100).unwrap();
        unfiltered_latencies.push(start.elapsed());
        assert_eq!(entries.len(), 100);
    }
    unfiltered_latencies.sort();

    println!(
        "[BENCH] list_memories_filtered(Fact,100)_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&filtered_latencies, 50.0)),
        fmt_dur(percentile(&filtered_latencies, 95.0)),
        fmt_dur(percentile(&filtered_latencies, 99.0)),
    );
    println!(
        "[BENCH] list_memories_unfiltered(None,100)_p50: {}, p95: {}, p99: {}",
        fmt_dur(percentile(&unfiltered_latencies, 50.0)),
        fmt_dur(percentile(&unfiltered_latencies, 95.0)),
        fmt_dur(percentile(&unfiltered_latencies, 99.0)),
    );
}

// ---------------------------------------------------------------------------
// 11. Batch metric insert vs individual insert
// ---------------------------------------------------------------------------

#[test]
fn bench_batch_vs_individual_metric_insert() {
    let providers = ["anthropic", "ollama", "openai"];
    let models = ["claude-sonnet-4-5", "llama-3-70b", "gpt-4"];

    // --- Individual inserts ---
    let db1 = Database::open_in_memory().unwrap();
    let individual_start = Instant::now();
    for i in 0..1000 {
        let metric = make_metric(i, providers[i % 3], models[i % 3]);
        db1.insert_metric(&metric).unwrap();
    }
    let individual_elapsed = individual_start.elapsed();

    // --- Batch insert ---
    let db2 = Database::open_in_memory().unwrap();
    let metrics: Vec<_> = (0..1000)
        .map(|i| make_metric(i, providers[i % 3], models[i % 3]))
        .collect();
    let batch_start = Instant::now();
    db2.batch_insert_metrics(&metrics).unwrap();
    let batch_elapsed = batch_start.elapsed();

    // Verify counts match.
    let sys1 = db1.system_metrics().unwrap();
    let sys2 = db2.system_metrics().unwrap();
    assert_eq!(sys1.total_invocations, 1000);
    assert_eq!(sys2.total_invocations, 1000);

    let speedup = individual_elapsed.as_secs_f64() / batch_elapsed.as_secs_f64();
    println!(
        "[BENCH] insert_1000_individual: {} ({})",
        fmt_dur(individual_elapsed),
        fmt_ops(1000, individual_elapsed),
    );
    println!(
        "[BENCH] insert_1000_batch: {} ({})",
        fmt_dur(batch_elapsed),
        fmt_ops(1000, batch_elapsed),
    );
    println!("[BENCH] batch_speedup: {speedup:.1}x");
    assert!(speedup > 1.0, "Batch insert should be faster than individual");
}
