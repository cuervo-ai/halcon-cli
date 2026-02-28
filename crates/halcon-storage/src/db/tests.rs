use super::*;
use chrono::Utc;
use uuid::Uuid;

use crate::memory::{MemoryEntry, MemoryEntryType};
use crate::trace::{TraceStep, TraceStepType};
use halcon_core::types::{DomainEvent, EventPayload, Session, TokenUsage};

    #[test]
    fn database_open_in_memory() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.path().to_str().unwrap(), ":memory:");
    }

    #[test]
    fn session_crud() {
        let db = Database::open_in_memory().unwrap();

        let session = Session::new(
            "test-model".to_string(),
            "test".to_string(),
            "/tmp".to_string(),
        );
        let id = session.id;

        // Save
        db.save_session(&session).unwrap();

        // Load
        let loaded = db.load_session(id).unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.model, "test-model");

        // List
        let sessions = db.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);

        // Delete
        db.delete_session(id).unwrap();
        let deleted = db.load_session(id).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn audit_hash_chain() {
        let db = Database::open_in_memory().unwrap();

        let event1 = DomainEvent::new(EventPayload::SessionStarted {
            session_id: Uuid::new_v4(),
        });
        let event2 = DomainEvent::new(EventPayload::SessionEnded {
            session_id: Uuid::new_v4(),
            total_usage: TokenUsage::default(),
        });

        db.append_audit_event(&event1).unwrap();
        db.append_audit_event(&event2).unwrap();

        // Verify chain: event2's previous_hash should be event1's hash
        let conn = db.conn.lock().unwrap();
        let hashes: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT previous_hash, hash FROM audit_log ORDER BY id")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap()
        };

        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes[0].0, "0"); // First event chains from "0"
        assert_eq!(hashes[1].0, hashes[0].1); // Second chains from first's hash
    }

    #[test]
    fn trace_step_append_and_load() {
        let db = Database::open_in_memory().unwrap();
        let session_id = Uuid::new_v4();

        let step0 = TraceStep {
            session_id,
            step_index: 0,
            step_type: TraceStepType::ModelRequest,
            data_json: r#"{"model":"echo","message_count":1}"#.to_string(),
            duration_ms: 0,
            timestamp: Utc::now(),
        };
        let step1 = TraceStep {
            session_id,
            step_index: 1,
            step_type: TraceStepType::ModelResponse,
            data_json: r#"{"text":"hello","stop_reason":"end_turn"}"#.to_string(),
            duration_ms: 42,
            timestamp: Utc::now(),
        };

        db.append_trace_step(&step0).unwrap();
        db.append_trace_step(&step1).unwrap();

        let steps = db.load_trace_steps(session_id).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].step_index, 0);
        assert_eq!(steps[0].step_type, TraceStepType::ModelRequest);
        assert_eq!(steps[1].step_index, 1);
        assert_eq!(steps[1].step_type, TraceStepType::ModelResponse);
        assert_eq!(steps[1].duration_ms, 42);
    }

    #[test]
    fn trace_step_ordering() {
        let db = Database::open_in_memory().unwrap();
        let session_id = Uuid::new_v4();

        // Insert out of order.
        for idx in [2, 0, 1] {
            let step = TraceStep {
                session_id,
                step_index: idx,
                step_type: TraceStepType::ToolCall,
                data_json: format!(r#"{{"index":{idx}}}"#),
                duration_ms: 0,
                timestamp: Utc::now(),
            };
            db.append_trace_step(&step).unwrap();
        }

        let steps = db.load_trace_steps(session_id).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].step_index, 0);
        assert_eq!(steps[1].step_index, 1);
        assert_eq!(steps[2].step_index, 2);
    }

    #[test]
    fn trace_step_unique_constraint() {
        let db = Database::open_in_memory().unwrap();
        let session_id = Uuid::new_v4();

        let step = TraceStep {
            session_id,
            step_index: 0,
            step_type: TraceStepType::ModelRequest,
            data_json: "{}".to_string(),
            duration_ms: 0,
            timestamp: Utc::now(),
        };

        db.append_trace_step(&step).unwrap();
        // Duplicate should fail.
        assert!(db.append_trace_step(&step).is_err());
    }

    #[test]
    fn trace_export_deterministic_structure() {
        let db = Database::open_in_memory().unwrap();
        let session_id = Uuid::new_v4();

        let step = TraceStep {
            session_id,
            step_index: 0,
            step_type: TraceStepType::ModelRequest,
            data_json: r#"{"model":"echo"}"#.to_string(),
            duration_ms: 10,
            timestamp: Utc::now(),
        };
        db.append_trace_step(&step).unwrap();

        let export = db.export_trace(session_id).unwrap();
        assert_eq!(export.session_id, session_id);
        assert_eq!(export.step_count, 1);
        assert_eq!(export.steps.len(), 1);
        assert_eq!(export.steps[0].step_type, TraceStepType::ModelRequest);

        // Should serialize to valid JSON.
        let json = serde_json::to_string_pretty(&export).unwrap();
        assert!(json.contains("model_request"));
    }

    #[test]
    fn trace_empty_session_returns_empty() {
        let db = Database::open_in_memory().unwrap();
        let session_id = Uuid::new_v4();

        let steps = db.load_trace_steps(session_id).unwrap();
        assert!(steps.is_empty());

        let export = db.export_trace(session_id).unwrap();
        assert_eq!(export.step_count, 0);
    }

    #[test]
    fn trace_steps_isolated_by_session() {
        let db = Database::open_in_memory().unwrap();
        let session_a = Uuid::new_v4();
        let session_b = Uuid::new_v4();

        db.append_trace_step(&TraceStep {
            session_id: session_a,
            step_index: 0,
            step_type: TraceStepType::ModelRequest,
            data_json: "{}".to_string(),
            duration_ms: 0,
            timestamp: Utc::now(),
        })
        .unwrap();

        db.append_trace_step(&TraceStep {
            session_id: session_b,
            step_index: 0,
            step_type: TraceStepType::ToolCall,
            data_json: "{}".to_string(),
            duration_ms: 0,
            timestamp: Utc::now(),
        })
        .unwrap();

        let steps_a = db.load_trace_steps(session_a).unwrap();
        assert_eq!(steps_a.len(), 1);
        assert_eq!(steps_a[0].step_type, TraceStepType::ModelRequest);

        let steps_b = db.load_trace_steps(session_b).unwrap();
        assert_eq!(steps_b.len(), 1);
        assert_eq!(steps_b[0].step_type, TraceStepType::ToolCall);
    }

    // --- Planning tests ---

    #[test]
    fn save_plan_steps_and_update_outcome_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let session_id = Uuid::new_v4();
        let plan_id = Uuid::new_v4();

        let plan = halcon_core::traits::ExecutionPlan {
            goal: "Fix the bug".into(),
            steps: vec![
                halcon_core::traits::PlanStep {
                    step_id: uuid::Uuid::new_v4(),
                    description: "Read the file".into(),
                    tool_name: Some("read_file".into()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: None,
                },
                halcon_core::traits::PlanStep {
                    step_id: uuid::Uuid::new_v4(),
                    description: "Edit the file".into(),
                    tool_name: Some("edit_file".into()),
                    parallel: false,
                    confidence: 0.8,
                    expected_args: None,
                    outcome: None,
                },
            ],
            requires_confirmation: false,
            plan_id,
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        };

        db.save_plan_steps(&session_id, &plan).unwrap();

        // Verify steps were inserted.
        let conn = db.conn.lock().unwrap();
        let count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM planning_steps WHERE plan_id = ?1",
                rusqlite::params![plan_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);

        // Verify goal and description.
        let (goal, desc): (String, String) = conn
            .query_row(
                "SELECT goal, description FROM planning_steps WHERE plan_id = ?1 AND step_index = 0",
                rusqlite::params![plan_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(goal, "Fix the bug");
        assert_eq!(desc, "Read the file");
        drop(conn);

        // Update outcome.
        db.update_plan_step_outcome(&plan_id, 0, "success", "Read OK")
            .unwrap();
        db.update_plan_step_outcome(&plan_id, 1, "failed", "File not writable")
            .unwrap();

        // Verify outcomes.
        let conn = db.conn.lock().unwrap();
        let (o1, d1): (String, String) = conn
            .query_row(
                "SELECT outcome, outcome_detail FROM planning_steps WHERE plan_id = ?1 AND step_index = 0",
                rusqlite::params![plan_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(o1, "success");
        assert_eq!(d1, "Read OK");

        let (o2, d2): (String, String) = conn
            .query_row(
                "SELECT outcome, outcome_detail FROM planning_steps WHERE plan_id = ?1 AND step_index = 1",
                rusqlite::params![plan_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(o2, "failed");
        assert_eq!(d2, "File not writable");
    }

    #[test]
    fn save_replan_preserves_parent_id() {
        let db = Database::open_in_memory().unwrap();
        let session_id = Uuid::new_v4();
        let original_plan_id = Uuid::new_v4();
        let replan_id = Uuid::new_v4();

        let original = halcon_core::traits::ExecutionPlan {
            goal: "Original goal".into(),
            steps: vec![halcon_core::traits::PlanStep {
                step_id: uuid::Uuid::new_v4(),
                description: "Step A".into(),
                tool_name: None,
                parallel: false,
                confidence: 0.5,
                expected_args: None,
                outcome: None,
            }],
            requires_confirmation: false,
            plan_id: original_plan_id,
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        };

        let replan = halcon_core::traits::ExecutionPlan {
            goal: "Replanned goal".into(),
            steps: vec![halcon_core::traits::PlanStep {
                step_id: uuid::Uuid::new_v4(),
                description: "Step B (replan)".into(),
                tool_name: Some("bash".into()),
                parallel: false,
                confidence: 0.7,
                expected_args: None,
                outcome: None,
            }],
            requires_confirmation: false,
            plan_id: replan_id,
            replan_count: 1,
            parent_plan_id: Some(original_plan_id),
            ..Default::default()
        };

        db.save_plan_steps(&session_id, &original).unwrap();
        db.save_plan_steps(&session_id, &replan).unwrap();

        // Verify replan has parent_plan_id.
        let conn = db.conn.lock().unwrap();
        let parent: String = conn
            .query_row(
                "SELECT parent_plan_id FROM planning_steps WHERE plan_id = ?1",
                rusqlite::params![replan_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent, original_plan_id.to_string());

        let replan_count: u32 = conn
            .query_row(
                "SELECT replan_count FROM planning_steps WHERE plan_id = ?1",
                rusqlite::params![replan_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(replan_count, 1);
    }

    // --- Memory tests ---

    fn make_memory_entry(content: &str, entry_type: MemoryEntryType) -> MemoryEntry {
        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(content.as_bytes()));
        MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type,
            content: content.to_string(),
            content_hash: hash,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        }
    }

    #[test]
    fn memory_insert_and_load() {
        let db = Database::open_in_memory().unwrap();
        let entry = make_memory_entry("Rust workspace has nine crates", MemoryEntryType::Fact);
        let id = entry.entry_id;

        let inserted = db.insert_memory(&entry).unwrap();
        assert!(inserted);

        let loaded = db.load_memory(id).unwrap().unwrap();
        assert_eq!(loaded.entry_id, id);
        assert_eq!(loaded.entry_type, MemoryEntryType::Fact);
        assert_eq!(loaded.content, "Rust workspace has nine crates");
        assert_eq!(loaded.relevance_score, 1.0);
    }

    #[test]
    fn memory_dedup_by_hash() {
        let db = Database::open_in_memory().unwrap();
        let entry1 = make_memory_entry("duplicate content", MemoryEntryType::Fact);
        let mut entry2 = make_memory_entry("duplicate content", MemoryEntryType::Fact);
        // Same content hash, different entry_id
        entry2.content_hash = entry1.content_hash.clone();

        assert!(db.insert_memory(&entry1).unwrap());
        assert!(!db.insert_memory(&entry2).unwrap()); // Duplicate, not inserted
    }

    #[test]
    fn memory_exists_by_hash() {
        let db = Database::open_in_memory().unwrap();
        let entry = make_memory_entry("unique content", MemoryEntryType::Decision);

        assert!(!db.memory_exists_by_hash(&entry.content_hash).unwrap());
        db.insert_memory(&entry).unwrap();
        assert!(db.memory_exists_by_hash(&entry.content_hash).unwrap());
    }

    #[test]
    fn memory_delete() {
        let db = Database::open_in_memory().unwrap();
        let entry = make_memory_entry("to delete", MemoryEntryType::Fact);
        let id = entry.entry_id;

        db.insert_memory(&entry).unwrap();
        assert!(db.load_memory(id).unwrap().is_some());

        let deleted = db.delete_memory(id).unwrap();
        assert!(deleted);
        assert!(db.load_memory(id).unwrap().is_none());

        // Delete non-existent returns false
        assert!(!db.delete_memory(id).unwrap());
    }

    #[test]
    fn memory_list_all() {
        let db = Database::open_in_memory().unwrap();
        db.insert_memory(&make_memory_entry("fact one", MemoryEntryType::Fact))
            .unwrap();
        db.insert_memory(&make_memory_entry("decision one", MemoryEntryType::Decision))
            .unwrap();
        db.insert_memory(&make_memory_entry("fact two", MemoryEntryType::Fact))
            .unwrap();

        let all = db.list_memories(None, 100).unwrap();
        assert_eq!(all.len(), 3);

        let facts = db.list_memories(Some(MemoryEntryType::Fact), 100).unwrap();
        assert_eq!(facts.len(), 2);

        let decisions = db
            .list_memories(Some(MemoryEntryType::Decision), 100)
            .unwrap();
        assert_eq!(decisions.len(), 1);
    }

    #[test]
    fn memory_list_respects_limit() {
        let db = Database::open_in_memory().unwrap();
        for i in 0..5 {
            db.insert_memory(&make_memory_entry(
                &format!("entry {i}"),
                MemoryEntryType::Fact,
            ))
            .unwrap();
        }

        let limited = db.list_memories(None, 3).unwrap();
        assert_eq!(limited.len(), 3);
    }

    #[test]
    fn memory_fts_search() {
        let db = Database::open_in_memory().unwrap();
        db.insert_memory(&make_memory_entry(
            "Rust workspace with nine crates for CLI tool",
            MemoryEntryType::Fact,
        ))
        .unwrap();
        db.insert_memory(&make_memory_entry(
            "Python script for data analysis",
            MemoryEntryType::CodeSnippet,
        ))
        .unwrap();
        db.insert_memory(&make_memory_entry(
            "Decision to use tokio async runtime",
            MemoryEntryType::Decision,
        ))
        .unwrap();

        // Search for "rust" should find the first entry
        let results = db.search_memory_fts("rust", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Rust"));

        // Search for "async" should find the decision
        let results = db.search_memory_fts("async", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("tokio"));

        // Search for nonexistent term
        let results = db.search_memory_fts("nonexistent_xyz", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn memory_fts_porter_stemming() {
        let db = Database::open_in_memory().unwrap();
        db.insert_memory(&make_memory_entry(
            "The implementation uses running processes",
            MemoryEntryType::Fact,
        ))
        .unwrap();

        // "run" should match "running" via Porter stemming
        let results = db.search_memory_fts("run", 10).unwrap();
        assert_eq!(results.len(), 1);

        // "implement" should match "implementation"
        let results = db.search_memory_fts("implement", 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn memory_stats_empty() {
        let db = Database::open_in_memory().unwrap();
        let stats = db.memory_stats().unwrap();
        assert_eq!(stats.total_entries, 0);
        assert!(stats.by_type.is_empty());
        assert!(stats.oldest_entry.is_none());
        assert!(stats.newest_entry.is_none());
    }

    #[test]
    fn memory_stats_populated() {
        let db = Database::open_in_memory().unwrap();
        db.insert_memory(&make_memory_entry("fact 1", MemoryEntryType::Fact))
            .unwrap();
        db.insert_memory(&make_memory_entry("fact 2", MemoryEntryType::Fact))
            .unwrap();
        db.insert_memory(&make_memory_entry("decision 1", MemoryEntryType::Decision))
            .unwrap();

        let stats = db.memory_stats().unwrap();
        assert_eq!(stats.total_entries, 3);
        assert!(!stats.by_type.is_empty());
        assert!(stats.oldest_entry.is_some());
        assert!(stats.newest_entry.is_some());

        // Facts should be the top type
        let fact_count = stats
            .by_type
            .iter()
            .find(|(t, _)| t == "fact")
            .map(|(_, c)| *c)
            .unwrap_or(0);
        assert_eq!(fact_count, 2);
    }

    #[test]
    fn memory_prune_expired() {
        let db = Database::open_in_memory().unwrap();

        // Insert an entry that expires in the past
        let mut expired = make_memory_entry("old entry", MemoryEntryType::Fact);
        expired.expires_at = Some(Utc::now() - chrono::Duration::hours(1));
        db.insert_memory(&expired).unwrap();

        // Insert a non-expiring entry
        db.insert_memory(&make_memory_entry("fresh entry", MemoryEntryType::Fact))
            .unwrap();

        let removed = db.prune_memories(0, Some(Utc::now())).unwrap();
        assert_eq!(removed, 1);

        let remaining = db.list_memories(None, 100).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].content, "fresh entry");
    }

    #[test]
    fn memory_prune_excess() {
        let db = Database::open_in_memory().unwrap();

        // Insert 5 entries with different relevance
        for i in 0..5 {
            let mut entry = make_memory_entry(&format!("entry {i}"), MemoryEntryType::Fact);
            entry.relevance_score = i as f64;
            db.insert_memory(&entry).unwrap();
        }

        // Prune to max 3 — should remove the 2 lowest relevance
        let removed = db.prune_memories(3, None).unwrap();
        assert_eq!(removed, 2);

        let remaining = db.list_memories(None, 100).unwrap();
        assert_eq!(remaining.len(), 3);
    }

    #[test]
    fn memory_with_session_id() {
        let db = Database::open_in_memory().unwrap();
        let session_id = Uuid::new_v4();
        let mut entry = make_memory_entry("session fact", MemoryEntryType::SessionSummary);
        entry.session_id = Some(session_id);

        db.insert_memory(&entry).unwrap();

        let loaded = db.load_memory(entry.entry_id).unwrap().unwrap();
        assert_eq!(loaded.session_id, Some(session_id));
        assert_eq!(loaded.entry_type, MemoryEntryType::SessionSummary);
    }

    #[test]
    fn memory_with_metadata() {
        let db = Database::open_in_memory().unwrap();
        let mut entry = make_memory_entry("tagged fact", MemoryEntryType::ProjectMeta);
        entry.metadata = serde_json::json!({
            "tags": ["rust", "cli"],
            "file": "Cargo.toml"
        });

        db.insert_memory(&entry).unwrap();

        let loaded = db.load_memory(entry.entry_id).unwrap().unwrap();
        assert_eq!(loaded.metadata["tags"][0], "rust");
        assert_eq!(loaded.metadata["file"], "Cargo.toml");
    }

    #[test]
    fn memory_load_nonexistent() {
        let db = Database::open_in_memory().unwrap();
        let result = db.load_memory(Uuid::new_v4()).unwrap();
        assert!(result.is_none());
    }

    // --- Cache tests ---

    fn make_cache_entry(key: &str, model: &str, text: &str) -> crate::cache::CacheEntry {
        crate::cache::CacheEntry {
            cache_key: key.to_string(),
            model: model.to_string(),
            response_text: text.to_string(),
            tool_calls_json: None,
            stop_reason: "end_turn".to_string(),
            usage_json: r#"{"input_tokens":10,"output_tokens":5}"#.to_string(),
            created_at: Utc::now(),
            expires_at: None,
            hit_count: 0,
        }
    }

    #[test]
    fn cache_insert_and_lookup() {
        let db = Database::open_in_memory().unwrap();
        let entry = make_cache_entry("key1", "claude", "Hello world");

        db.insert_cache_entry(&entry).unwrap();

        let found = db.lookup_cache("key1").unwrap().unwrap();
        assert_eq!(found.cache_key, "key1");
        assert_eq!(found.model, "claude");
        assert_eq!(found.response_text, "Hello world");
        // hit_count in returned entry is pre-increment (0), DB is updated to 1.
        assert_eq!(found.hit_count, 0);
    }

    #[test]
    fn cache_miss_returns_none() {
        let db = Database::open_in_memory().unwrap();
        let result = db.lookup_cache("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn cache_expired_entry_returns_none() {
        let db = Database::open_in_memory().unwrap();
        let mut entry = make_cache_entry("expired_key", "claude", "old response");
        entry.expires_at = Some(Utc::now() - chrono::Duration::hours(1));

        db.insert_cache_entry(&entry).unwrap();

        let result = db.lookup_cache("expired_key").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn lookup_cache_ttl_in_where() {
        let db = Database::open_in_memory().unwrap();
        let mut entry = make_cache_entry("ttl_key", "claude", "response");
        entry.expires_at = Some(Utc::now() - chrono::Duration::hours(1));

        db.insert_cache_entry(&entry).unwrap();

        // Lookup should return None (expired via WHERE clause), no separate DELETE.
        let result = db.lookup_cache("ttl_key").unwrap();
        assert!(result.is_none());

        // Entry still exists in table (cleaned up by prune_cache, not lookup).
        let conn = db.conn.lock().unwrap();
        let count: u32 = conn
            .query_row("SELECT COUNT(*) FROM response_cache WHERE cache_key = 'ttl_key'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "Expired entry should still exist (cleaned by prune_cache)");
    }

    #[test]
    fn cache_hit_count_increments() {
        let db = Database::open_in_memory().unwrap();
        let entry = make_cache_entry("popular", "claude", "cached response");

        db.insert_cache_entry(&entry).unwrap();

        // First lookup: returns pre-increment (0), DB updated to 1.
        let first = db.lookup_cache("popular").unwrap().unwrap();
        assert_eq!(first.hit_count, 0);

        // Second lookup: returns pre-increment (1), DB updated to 2.
        let second = db.lookup_cache("popular").unwrap().unwrap();
        assert_eq!(second.hit_count, 1);

        // Third lookup: returns 2, DB updated to 3.
        let third = db.lookup_cache("popular").unwrap().unwrap();
        assert_eq!(third.hit_count, 2);
    }

    #[test]
    fn cache_replace_on_duplicate_key() {
        let db = Database::open_in_memory().unwrap();
        let entry1 = make_cache_entry("dup_key", "claude", "first response");
        let entry2 = make_cache_entry("dup_key", "claude", "second response");

        db.insert_cache_entry(&entry1).unwrap();
        db.insert_cache_entry(&entry2).unwrap();

        let found = db.lookup_cache("dup_key").unwrap().unwrap();
        assert_eq!(found.response_text, "second response");
    }

    #[test]
    fn cache_with_tool_calls() {
        let db = Database::open_in_memory().unwrap();
        let mut entry = make_cache_entry("tools_key", "claude", "I'll read that file");
        entry.tool_calls_json = Some(r#"[{"id":"t1","name":"file_read","input":{"path":"a.rs"}}]"#.to_string());
        entry.stop_reason = "tool_use".to_string();

        db.insert_cache_entry(&entry).unwrap();

        let found = db.lookup_cache("tools_key").unwrap().unwrap();
        assert!(found.tool_calls_json.is_some());
        assert_eq!(found.stop_reason, "tool_use");
    }

    #[test]
    fn cache_clear() {
        let db = Database::open_in_memory().unwrap();
        db.insert_cache_entry(&make_cache_entry("k1", "m", "r1")).unwrap();
        db.insert_cache_entry(&make_cache_entry("k2", "m", "r2")).unwrap();

        let removed = db.clear_cache().unwrap();
        assert_eq!(removed, 2);

        let stats = db.cache_stats().unwrap();
        assert_eq!(stats.total_entries, 0);
    }

    #[test]
    fn cache_prune_expired() {
        let db = Database::open_in_memory().unwrap();

        let mut expired = make_cache_entry("old", "m", "old response");
        expired.expires_at = Some(Utc::now() - chrono::Duration::hours(1));
        db.insert_cache_entry(&expired).unwrap();

        db.insert_cache_entry(&make_cache_entry("fresh", "m", "new response")).unwrap();

        let removed = db.prune_cache(0).unwrap();
        assert_eq!(removed, 1);

        let stats = db.cache_stats().unwrap();
        assert_eq!(stats.total_entries, 1);
    }

    #[test]
    fn cache_prune_excess() {
        let db = Database::open_in_memory().unwrap();
        for i in 0..5 {
            db.insert_cache_entry(&make_cache_entry(&format!("k{i}"), "m", &format!("r{i}"))).unwrap();
        }

        let removed = db.prune_cache(3).unwrap();
        assert_eq!(removed, 2);

        let stats = db.cache_stats().unwrap();
        assert_eq!(stats.total_entries, 3);
    }

    #[test]
    fn cache_stats_empty() {
        let db = Database::open_in_memory().unwrap();
        let stats = db.cache_stats().unwrap();
        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.total_hits, 0);
        assert!(stats.oldest_entry.is_none());
    }

    #[test]
    fn cache_stats_populated() {
        let db = Database::open_in_memory().unwrap();
        db.insert_cache_entry(&make_cache_entry("k1", "m", "r1")).unwrap();
        db.insert_cache_entry(&make_cache_entry("k2", "m", "r2")).unwrap();

        // Generate some hits (each lookup increments DB hit_count by 1)
        db.lookup_cache("k1").unwrap();
        db.lookup_cache("k1").unwrap();
        db.lookup_cache("k2").unwrap();

        let stats = db.cache_stats().unwrap();
        assert_eq!(stats.total_entries, 2);
        // k1: 2 lookups, k2: 1 lookup => total 3
        assert_eq!(stats.total_hits, 3);
        assert!(stats.oldest_entry.is_some());
        assert!(stats.newest_entry.is_some());
    }

    #[test]
    fn cache_stats_single_query_edge_cases() {
        let db = Database::open_in_memory().unwrap();

        // Empty table: should return zeros and None dates.
        let stats = db.cache_stats().unwrap();
        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.total_hits, 0);
        assert!(stats.oldest_entry.is_none());
        assert!(stats.newest_entry.is_none());

        // Single entry: oldest == newest.
        db.insert_cache_entry(&make_cache_entry("k1", "m", "r1")).unwrap();
        let stats = db.cache_stats().unwrap();
        assert_eq!(stats.total_entries, 1);
        assert!(stats.oldest_entry.is_some());
        assert_eq!(stats.oldest_entry, stats.newest_entry);
    }

    #[test]
    fn memory_stats_consolidated_edge_cases() {
        let db = Database::open_in_memory().unwrap();

        // Empty table.
        let stats = db.memory_stats().unwrap();
        assert_eq!(stats.total_entries, 0);
        assert!(stats.by_type.is_empty());
        assert!(stats.oldest_entry.is_none());
        assert!(stats.newest_entry.is_none());

        // Single entry: oldest == newest.
        db.insert_memory(&make_memory_entry("solo", MemoryEntryType::Fact)).unwrap();
        let stats = db.memory_stats().unwrap();
        assert_eq!(stats.total_entries, 1);
        assert!(stats.oldest_entry.is_some());
        assert_eq!(stats.oldest_entry, stats.newest_entry);
        assert_eq!(stats.by_type.len(), 1);
    }

    // ---- Metrics tests ----

    fn make_metric(provider: &str, model: &str, latency_ms: u64, cost: f64, success: bool) -> crate::metrics::InvocationMetric {
        crate::metrics::InvocationMetric {
            provider: provider.to_string(),
            model: model.to_string(),
            latency_ms,
            input_tokens: 100,
            output_tokens: 50,
            estimated_cost_usd: cost,
            success,
            stop_reason: if success { "end_turn" } else { "error" }.to_string(),
            session_id: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn metric_insert_and_system_metrics() {
        let db = Database::open_in_memory().unwrap();

        db.insert_metric(&make_metric("anthropic", "claude", 500, 0.001, true)).unwrap();
        db.insert_metric(&make_metric("anthropic", "claude", 1500, 0.003, true)).unwrap();
        db.insert_metric(&make_metric("ollama", "llama", 200, 0.0, true)).unwrap();

        let sys = db.system_metrics().unwrap();
        assert_eq!(sys.total_invocations, 3);
        assert_eq!(sys.models.len(), 2);
        assert!(sys.total_cost_usd > 0.003);
    }

    #[test]
    fn model_stats_aggregation() {
        let db = Database::open_in_memory().unwrap();

        db.insert_metric(&make_metric("anthropic", "claude", 500, 0.001, true)).unwrap();
        db.insert_metric(&make_metric("anthropic", "claude", 1000, 0.002, true)).unwrap();
        db.insert_metric(&make_metric("anthropic", "claude", 2000, 0.003, false)).unwrap();

        let stats = db.model_stats("anthropic", "claude").unwrap();
        assert_eq!(stats.total_invocations, 3);
        assert_eq!(stats.successful_invocations, 2);
        assert!((stats.success_rate - 2.0 / 3.0).abs() < 0.01);
        assert!((stats.avg_latency_ms - 1166.67).abs() < 1.0);
        assert_eq!(stats.total_tokens, 450); // 3 * (100+50)
        assert!((stats.total_cost_usd - 0.006).abs() < 0.0001);
    }

    #[test]
    fn model_stats_p95_latency() {
        let db = Database::open_in_memory().unwrap();

        // Insert 20 metrics with increasing latency.
        for i in 1..=20 {
            db.insert_metric(&make_metric("a", "m", i * 100, 0.001, true)).unwrap();
        }

        let stats = db.model_stats("a", "m").unwrap();
        // P95 of [100..2000] = 1900 (19th value at 0-indexed offset 18).
        assert_eq!(stats.p95_latency_ms, 1900);
    }

    #[test]
    fn model_stats_empty() {
        let db = Database::open_in_memory().unwrap();
        let stats = db.model_stats("nonexistent", "nothing").unwrap();
        assert_eq!(stats.total_invocations, 0);
        assert_eq!(stats.success_rate, 0.0);
        assert_eq!(stats.avg_latency_ms, 0.0);
    }

    #[test]
    fn system_metrics_empty() {
        let db = Database::open_in_memory().unwrap();
        let sys = db.system_metrics().unwrap();
        assert_eq!(sys.total_invocations, 0);
        assert!(sys.models.is_empty());
    }

    #[test]
    fn system_metrics_group_by_multiple_models() {
        let db = Database::open_in_memory().unwrap();

        // 3 different provider/model combos
        db.insert_metric(&make_metric("anthropic", "claude-3", 500, 0.01, true)).unwrap();
        db.insert_metric(&make_metric("anthropic", "claude-3", 600, 0.02, true)).unwrap();
        db.insert_metric(&make_metric("anthropic", "haiku", 100, 0.001, true)).unwrap();
        db.insert_metric(&make_metric("ollama", "llama", 200, 0.0, true)).unwrap();
        db.insert_metric(&make_metric("ollama", "llama", 300, 0.0, false)).unwrap();

        let sys = db.system_metrics().unwrap();
        assert_eq!(sys.total_invocations, 5);
        assert_eq!(sys.models.len(), 3);
        assert!((sys.total_cost_usd - 0.031).abs() < 0.0001);
        assert_eq!(sys.total_tokens, 750); // 5 * (100+50)

        // Verify per-model stats are correct
        let claude3 = sys.models.iter().find(|m| m.model == "claude-3").unwrap();
        assert_eq!(claude3.total_invocations, 2);
        assert_eq!(claude3.successful_invocations, 2);
        assert!((claude3.success_rate - 1.0).abs() < 0.01);

        let llama = sys.models.iter().find(|m| m.model == "llama").unwrap();
        assert_eq!(llama.total_invocations, 2);
        assert_eq!(llama.successful_invocations, 1);
        assert!((llama.success_rate - 0.5).abs() < 0.01);

        let haiku = sys.models.iter().find(|m| m.model == "haiku").unwrap();
        assert_eq!(haiku.total_invocations, 1);
    }

    #[test]
    fn prune_metrics_by_age() {
        let db = Database::open_in_memory().unwrap();

        let mut old = make_metric("a", "m", 100, 0.001, true);
        old.created_at = Utc::now() - chrono::Duration::days(31);
        db.insert_metric(&old).unwrap();

        db.insert_metric(&make_metric("a", "m", 200, 0.001, true)).unwrap();

        let removed = db.prune_metrics(30).unwrap();
        assert_eq!(removed, 1);

        let sys = db.system_metrics().unwrap();
        assert_eq!(sys.total_invocations, 1);
    }

    // ---- Resilience events ----

    fn make_resilience_event(
        provider: &str,
        event_type: &str,
    ) -> crate::resilience::ResilienceEvent {
        crate::resilience::ResilienceEvent {
            provider: provider.to_string(),
            event_type: event_type.to_string(),
            from_state: Some("closed".to_string()),
            to_state: Some("open".to_string()),
            score: None,
            details: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn insert_and_query_resilience_event() {
        let db = Database::open_in_memory().unwrap();

        let event = make_resilience_event("anthropic", "breaker_trip");
        db.insert_resilience_event(&event).unwrap();

        let events = db.resilience_events(None, None, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].provider, "anthropic");
        assert_eq!(events[0].event_type, "breaker_trip");
        assert_eq!(events[0].from_state.as_deref(), Some("closed"));
        assert_eq!(events[0].to_state.as_deref(), Some("open"));
    }

    #[test]
    fn query_resilience_events_filters() {
        let db = Database::open_in_memory().unwrap();

        db.insert_resilience_event(&make_resilience_event("a", "breaker_trip")).unwrap();
        db.insert_resilience_event(&make_resilience_event("b", "saturation")).unwrap();
        db.insert_resilience_event(&make_resilience_event("a", "recovery")).unwrap();

        // Filter by provider.
        let a_events = db.resilience_events(Some("a"), None, 10).unwrap();
        assert_eq!(a_events.len(), 2);

        // Filter by type.
        let trips = db.resilience_events(None, Some("breaker_trip"), 10).unwrap();
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].provider, "a");

        // Filter by both.
        let a_recovery = db.resilience_events(Some("a"), Some("recovery"), 10).unwrap();
        assert_eq!(a_recovery.len(), 1);
    }

    #[test]
    fn prune_resilience_events_by_age() {
        let db = Database::open_in_memory().unwrap();

        let mut old = make_resilience_event("a", "breaker_trip");
        old.created_at = Utc::now() - chrono::Duration::days(10);
        db.insert_resilience_event(&old).unwrap();

        db.insert_resilience_event(&make_resilience_event("a", "recovery")).unwrap();

        let removed = db.prune_resilience_events(7).unwrap();
        assert_eq!(removed, 1);

        let remaining = db.resilience_events(None, None, 10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].event_type, "recovery");
    }

    #[test]
    fn provider_metrics_windowed_empty() {
        let db = Database::open_in_memory().unwrap();

        let metrics = db.provider_metrics_windowed("unknown", 60).unwrap();
        assert_eq!(metrics.total_invocations, 0);
        assert_eq!(metrics.provider, "unknown");
    }

    #[test]
    fn provider_metrics_windowed_aggregates() {
        let db = Database::open_in_memory().unwrap();

        // Insert mixed success/failure metrics.
        for _ in 0..3 {
            db.insert_metric(&make_metric("prov", "m", 200, 0.001, true)).unwrap();
        }
        let mut fail = make_metric("prov", "m", 5000, 0.001, false);
        fail.stop_reason = "error".to_string();
        db.insert_metric(&fail).unwrap();

        let mut timeout = make_metric("prov", "m", 30000, 0.0, false);
        timeout.stop_reason = "timeout".to_string();
        db.insert_metric(&timeout).unwrap();

        let metrics = db.provider_metrics_windowed("prov", 60).unwrap();
        assert_eq!(metrics.total_invocations, 5);
        assert_eq!(metrics.successful_invocations, 3);
        assert_eq!(metrics.failed_invocations, 2);
        assert_eq!(metrics.timeout_count, 1);
        assert!((metrics.error_rate - 0.4).abs() < 0.01);
        assert!((metrics.timeout_rate - 0.2).abs() < 0.01);
    }

    #[test]
    fn busy_timeout_pragma_set() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn.lock().unwrap();
        // In-memory DB doesn't set busy_timeout via our open() path (uses open_in_memory),
        // but we can verify the pragma works on a file-based DB.
        // For in-memory, just verify we can set and query it.
        conn.execute_batch("PRAGMA busy_timeout=5000;").unwrap();
        let timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
    }

    #[test]
    fn session_metrics_roundtrip() {
        let db = Database::open_in_memory().unwrap();

        let mut session = Session::new("model".into(), "provider".into(), "/tmp".into());
        session.tool_invocations = 10;
        session.agent_rounds = 5;
        session.total_latency_ms = 3000;
        session.estimated_cost_usd = 0.123;
        let id = session.id;

        db.save_session(&session).unwrap();
        let loaded = db.load_session(id).unwrap().unwrap();
        assert_eq!(loaded.tool_invocations, 10);
        assert_eq!(loaded.agent_rounds, 5);
        assert_eq!(loaded.total_latency_ms, 3000);
        assert!((loaded.estimated_cost_usd - 0.123).abs() < 0.001);
    }

    #[test]
    fn session_resume_preserves_history() {
        use halcon_core::types::{ChatMessage, MessageContent, Role};

        let db = Database::open_in_memory().unwrap();

        let mut session = Session::new("claude".into(), "anthropic".into(), "/workspace".into());
        session.add_message(ChatMessage {
            role: Role::User,
            content: MessageContent::Text("hello".into()),
        });
        session.add_message(ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text("hi there!".into()),
        });
        session.tool_invocations = 3;
        session.agent_rounds = 2;
        session.total_latency_ms = 800;
        session.estimated_cost_usd = 0.005;
        session.total_usage = TokenUsage {
            input_tokens: 50,
            output_tokens: 30,
            ..Default::default()
        };
        let id = session.id;

        db.save_session(&session).unwrap();

        // Simulate resume: load and verify complete state.
        let resumed = db.load_session(id).unwrap().unwrap();
        assert_eq!(resumed.messages.len(), 2);
        assert_eq!(resumed.model, "claude");
        assert_eq!(resumed.provider, "anthropic");
        assert_eq!(resumed.working_directory, "/workspace");
        assert_eq!(resumed.total_usage.input_tokens, 50);
        assert_eq!(resumed.total_usage.output_tokens, 30);
        assert_eq!(resumed.tool_invocations, 3);
        assert_eq!(resumed.agent_rounds, 2);
        assert_eq!(resumed.total_latency_ms, 800);
        assert!((resumed.estimated_cost_usd - 0.005).abs() < 0.001);
    }

    #[test]
    fn session_update_increments_metrics() {
        let db = Database::open_in_memory().unwrap();

        let mut session = Session::new("echo".into(), "echo".into(), "/tmp".into());
        let id = session.id;
        db.save_session(&session).unwrap();

        // Simulate agent loop updating metrics.
        session.tool_invocations += 2;
        session.agent_rounds += 1;
        session.total_latency_ms += 500;
        session.estimated_cost_usd += 0.01;
        db.save_session(&session).unwrap();

        let loaded = db.load_session(id).unwrap().unwrap();
        assert_eq!(loaded.tool_invocations, 2);
        assert_eq!(loaded.agent_rounds, 1);
        assert_eq!(loaded.total_latency_ms, 500);

        // Simulate second round of updates.
        session.tool_invocations += 3;
        session.agent_rounds += 1;
        session.total_latency_ms += 300;
        db.save_session(&session).unwrap();

        let loaded2 = db.load_session(id).unwrap().unwrap();
        assert_eq!(loaded2.tool_invocations, 5);
        assert_eq!(loaded2.agent_rounds, 2);
        assert_eq!(loaded2.total_latency_ms, 800);
    }

    #[test]
    fn cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &a);
        assert!((sim - 1.0).abs() < 0.001, "identical vectors should have similarity 1.0, got {sim}");
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 0.001, "orthogonal vectors should have similarity 0.0, got {sim}");
    }

    #[test]
    fn cosine_similarity_opposite() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 0.001, "opposite vectors should have similarity -1.0, got {sim}");
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![0.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 0.001, "zero vector should have similarity 0.0, got {sim}");
    }

    #[test]
    fn embedding_roundtrip() {
        let db = Database::open_in_memory().unwrap();

        let entry = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: MemoryEntryType::Fact,
            content: "Test embedding storage".to_string(),
            content_hash: "embed_hash_1".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        db.insert_memory(&entry).unwrap();

        let embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        db.update_entry_embedding(&entry.entry_id.to_string(), &embedding, "test-model")
            .unwrap();

        // Search by embedding should find the entry.
        let query_vec = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let results = db.search_memory_by_embedding(&query_vec, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry_id, entry.entry_id);
    }

    #[test]
    fn save_and_load_episode() {
        let db = Database::open_in_memory().unwrap();

        let episode = crate::memory::MemoryEpisode {
            episode_id: "ep-001".to_string(),
            session_id: Some("sess-001".to_string()),
            title: "Fix authentication bug".to_string(),
            summary: Some("Resolved JWT expiry issue".to_string()),
            started_at: Utc::now(),
            ended_at: None,
            metadata: serde_json::json!({"tags": ["auth", "jwt"]}),
        };
        db.save_episode(&episode).unwrap();

        let loaded = db.load_episode("ep-001").unwrap().unwrap();
        assert_eq!(loaded.episode_id, "ep-001");
        assert_eq!(loaded.title, "Fix authentication bug");
        assert_eq!(loaded.summary, Some("Resolved JWT expiry issue".to_string()));
        assert_eq!(loaded.session_id, Some("sess-001".to_string()));
    }

    #[test]
    fn link_entry_to_episode_and_load() {
        let db = Database::open_in_memory().unwrap();

        // Create episode.
        let episode = crate::memory::MemoryEpisode {
            episode_id: "ep-002".to_string(),
            session_id: None,
            title: "Test episode".to_string(),
            summary: None,
            started_at: Utc::now(),
            ended_at: None,
            metadata: serde_json::json!({}),
        };
        db.save_episode(&episode).unwrap();

        // Create memory entries.
        let entry1 = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: MemoryEntryType::Fact,
            content: "First entry in episode".to_string(),
            content_hash: "link_hash_1".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        let entry2 = MemoryEntry {
            entry_id: Uuid::new_v4(),
            session_id: None,
            entry_type: MemoryEntryType::Decision,
            content: "Second entry in episode".to_string(),
            content_hash: "link_hash_2".to_string(),
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
            expires_at: None,
            relevance_score: 1.0,
        };
        db.insert_memory(&entry1).unwrap();
        db.insert_memory(&entry2).unwrap();

        // Link entries to episode.
        db.link_entry_to_episode(&entry1.entry_id.to_string(), "ep-002", 0)
            .unwrap();
        db.link_entry_to_episode(&entry2.entry_id.to_string(), "ep-002", 1)
            .unwrap();

        // Load episode entries.
        let entries = db.load_episode_entries("ep-002").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content, "First entry in episode");
        assert_eq!(entries[1].content, "Second entry in episode");
    }

    // ---- Tool execution metrics ----

    fn make_tool_metric(
        tool_name: &str,
        duration_ms: u64,
        success: bool,
        is_parallel: bool,
    ) -> crate::metrics::ToolExecutionMetric {
        crate::metrics::ToolExecutionMetric {
            tool_name: tool_name.to_string(),
            session_id: None,
            duration_ms,
            success,
            is_parallel,
            input_summary: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn insert_tool_metric_roundtrip() {
        let db = Database::open_in_memory().unwrap();

        db.insert_tool_metric(&make_tool_metric("bash", 150, true, false))
            .unwrap();
        db.insert_tool_metric(&make_tool_metric("bash", 250, true, false))
            .unwrap();
        db.insert_tool_metric(&make_tool_metric("read_file", 10, true, true))
            .unwrap();

        let stats = db.tool_stats("bash").unwrap();
        assert_eq!(stats.total_executions, 2);
        assert!((stats.avg_duration_ms - 200.0).abs() < 1.0);
        assert!((stats.success_rate - 1.0).abs() < 0.01);
    }

    #[test]
    fn tool_stats_aggregation() {
        let db = Database::open_in_memory().unwrap();

        db.insert_tool_metric(&make_tool_metric("bash", 100, true, false))
            .unwrap();
        db.insert_tool_metric(&make_tool_metric("bash", 200, true, false))
            .unwrap();
        db.insert_tool_metric(&make_tool_metric("bash", 300, false, false))
            .unwrap();

        let stats = db.tool_stats("bash").unwrap();
        assert_eq!(stats.total_executions, 3);
        assert!((stats.avg_duration_ms - 200.0).abs() < 1.0);
        assert!((stats.success_rate - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn tool_stats_empty() {
        let db = Database::open_in_memory().unwrap();
        let stats = db.tool_stats("nonexistent").unwrap();
        assert_eq!(stats.total_executions, 0);
        assert_eq!(stats.avg_duration_ms, 0.0);
        assert_eq!(stats.success_rate, 0.0);
    }

    #[test]
    fn top_tool_stats_ordering() {
        let db = Database::open_in_memory().unwrap();

        // bash: 3 executions
        for _ in 0..3 {
            db.insert_tool_metric(&make_tool_metric("bash", 100, true, false))
                .unwrap();
        }
        // read_file: 5 executions
        for _ in 0..5 {
            db.insert_tool_metric(&make_tool_metric("read_file", 10, true, true))
                .unwrap();
        }
        // grep: 1 execution
        db.insert_tool_metric(&make_tool_metric("grep", 50, true, true))
            .unwrap();

        let top = db.top_tool_stats(2).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].tool_name, "read_file"); // most executions
        assert_eq!(top[0].total_executions, 5);
        assert_eq!(top[1].tool_name, "bash");
        assert_eq!(top[1].total_executions, 3);
    }

    #[test]
    fn audit_event_with_session_id() {
        let db = Database::open_in_memory().unwrap();

        let event = DomainEvent::new(EventPayload::SessionStarted {
            session_id: Uuid::new_v4(),
        });
        db.append_audit_event_with_session(&event, Some("sess-123"))
            .unwrap();

        // Verify session_id is stored and queryable.
        let conn = db.conn.lock().unwrap();
        let stored_session: Option<String> = conn
            .query_row(
                "SELECT session_id FROM audit_log WHERE event_id = ?1",
                rusqlite::params![event.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_session.as_deref(), Some("sess-123"));
    }

    // --- Agent Tasks ---

    #[test]
    fn agent_task_save_and_load() {
        let db = Database::open_in_memory().unwrap();
        let orch_id = Uuid::new_v4().to_string();
        let task_id = Uuid::new_v4().to_string();

        db.save_agent_task(
            &task_id, &orch_id, "sess-1", "Chat", "Do something",
            "running", 0, 0, 0.0, 0, 0, None, None,
        ).unwrap();

        let tasks = db.load_agent_tasks(&orch_id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, task_id);
        assert_eq!(tasks[0].status, "running");
        assert_eq!(tasks[0].instruction, "Do something");
        assert_eq!(tasks[0].agent_type, "Chat");
    }

    #[test]
    fn agent_task_update_status() {
        let db = Database::open_in_memory().unwrap();
        let orch_id = Uuid::new_v4().to_string();
        let task_id = Uuid::new_v4().to_string();

        db.save_agent_task(
            &task_id, &orch_id, "sess-1", "Chat", "Test task",
            "running", 0, 0, 0.0, 0, 0, None, None,
        ).unwrap();

        db.update_agent_task_status(
            &task_id, "completed",
            100, 50, 0.005, 1500, 3,
            None, Some("Task output text"),
        ).unwrap();

        let tasks = db.load_agent_tasks(&orch_id).unwrap();
        assert_eq!(tasks[0].status, "completed");
        assert_eq!(tasks[0].input_tokens, 100);
        assert_eq!(tasks[0].output_tokens, 50);
        assert!((tasks[0].cost_usd - 0.005).abs() < 0.001);
        assert_eq!(tasks[0].latency_ms, 1500);
        assert_eq!(tasks[0].rounds, 3);
        assert_eq!(tasks[0].output_text.as_deref(), Some("Task output text"));
        assert!(tasks[0].completed_at.is_some());
    }

    #[test]
    fn agent_task_load_filters_by_orchestrator() {
        let db = Database::open_in_memory().unwrap();
        let orch1 = Uuid::new_v4().to_string();
        let orch2 = Uuid::new_v4().to_string();

        db.save_agent_task(
            &Uuid::new_v4().to_string(), &orch1, "s1", "Chat", "Task A",
            "completed", 10, 5, 0.001, 100, 1, None, None,
        ).unwrap();
        db.save_agent_task(
            &Uuid::new_v4().to_string(), &orch2, "s2", "Chat", "Task B",
            "completed", 20, 10, 0.002, 200, 2, None, None,
        ).unwrap();
        db.save_agent_task(
            &Uuid::new_v4().to_string(), &orch1, "s3", "Chat", "Task C",
            "running", 0, 0, 0.0, 0, 0, None, None,
        ).unwrap();

        let orch1_tasks = db.load_agent_tasks(&orch1).unwrap();
        assert_eq!(orch1_tasks.len(), 2);

        let orch2_tasks = db.load_agent_tasks(&orch2).unwrap();
        assert_eq!(orch2_tasks.len(), 1);
    }

    #[test]
    fn agent_task_failed_with_error() {
        let db = Database::open_in_memory().unwrap();
        let orch_id = Uuid::new_v4().to_string();
        let task_id = Uuid::new_v4().to_string();

        db.save_agent_task(
            &task_id, &orch_id, "sess-1", "Chat", "Failing task",
            "running", 0, 0, 0.0, 0, 0, None, None,
        ).unwrap();

        db.update_agent_task_status(
            &task_id, "failed",
            50, 25, 0.002, 800, 1,
            Some("Provider timeout"), None,
        ).unwrap();

        let tasks = db.load_agent_tasks(&orch_id).unwrap();
        assert_eq!(tasks[0].status, "failed");
        assert_eq!(tasks[0].error_message.as_deref(), Some("Provider timeout"));
    }

    #[test]
    fn count_recent_orchestrator_runs() {
        let db = Database::open_in_memory().unwrap();
        let orch1 = Uuid::new_v4().to_string();
        let orch2 = Uuid::new_v4().to_string();

        // Two tasks in orch1, one in orch2.
        db.save_agent_task(
            &Uuid::new_v4().to_string(), &orch1, "s1", "Chat", "A",
            "completed", 0, 0, 0.0, 0, 0, None, None,
        ).unwrap();
        db.save_agent_task(
            &Uuid::new_v4().to_string(), &orch1, "s2", "Chat", "B",
            "completed", 0, 0, 0.0, 0, 0, None, None,
        ).unwrap();
        db.save_agent_task(
            &Uuid::new_v4().to_string(), &orch2, "s3", "Chat", "C",
            "running", 0, 0, 0.0, 0, 0, None, None,
        ).unwrap();

        let count = db.count_recent_orchestrator_runs(7).unwrap();
        assert_eq!(count, 2, "should count 2 distinct orchestrator IDs");
    }

    // --- Phase 18: max_step_index tests ---

    #[test]
    fn max_step_index_empty_session() {
        let db = Database::open_in_memory().unwrap();
        let sid = Uuid::new_v4();
        let max = db.max_step_index(sid).unwrap();
        assert_eq!(max, None, "empty session should return None");
    }

    #[test]
    fn max_step_index_after_steps() {
        let db = Database::open_in_memory().unwrap();
        let sid = Uuid::new_v4();

        for i in 0..3 {
            let step = TraceStep {
                session_id: sid,
                step_index: i,
                step_type: TraceStepType::ModelRequest,
                data_json: "{}".to_string(),
                duration_ms: 10,
                timestamp: Utc::now(),
            };
            db.append_trace_step(&step).unwrap();
        }

        let max = db.max_step_index(sid).unwrap();
        assert_eq!(max, Some(2), "max step_index should be 2 after 3 steps");
    }

    #[test]
    fn max_step_index_multi_message_no_collision() {
        let db = Database::open_in_memory().unwrap();
        let sid = Uuid::new_v4();

        // Simulate first message: steps 0, 1
        for i in 0..2 {
            let step = TraceStep {
                session_id: sid,
                step_index: i,
                step_type: TraceStepType::ModelRequest,
                data_json: "{}".to_string(),
                duration_ms: 10,
                timestamp: Utc::now(),
            };
            db.append_trace_step(&step).unwrap();
        }

        // Get next index
        let next = db.max_step_index(sid).unwrap().map(|m| m + 1).unwrap_or(0);
        assert_eq!(next, 2);

        // Simulate second message: steps starting at 2
        for i in next..next + 2 {
            let step = TraceStep {
                session_id: sid,
                step_index: i,
                step_type: TraceStepType::ModelResponse,
                data_json: "{}".to_string(),
                duration_ms: 10,
                timestamp: Utc::now(),
            };
            db.append_trace_step(&step).unwrap();
        }

        let all_steps = db.load_trace_steps(sid).unwrap();
        assert_eq!(all_steps.len(), 4, "should have 4 total steps");
        // Verify all indices are unique
        let indices: Vec<u32> = all_steps.iter().map(|s| s.step_index).collect();
        assert_eq!(indices, vec![0, 1, 2, 3]);
    }

    #[test]
    fn conn_pragma_queries_return_values() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn().unwrap();

        // page_count returns an integer — should work with i64 then to_string.
        let page_count: i64 = conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .unwrap();
        assert!(page_count >= 0, "page_count should be non-negative");

        // page_size returns an integer.
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        assert!(page_size > 0, "page_size should be positive (typically 4096)");

        // journal_mode returns a string.
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert!(!journal.is_empty(), "journal_mode should be non-empty");
    }

    // ─── HMAC key tests (B1) ──────────────────────────────────────────────────

    #[test]
    fn hmac_key_is_32_bytes_on_fresh_db() {
        let db = Database::open_in_memory().unwrap();
        // Non-zero: extremely unlikely to be all zeros from a CSPRNG.
        assert_ne!(db.audit_hmac_key, [0u8; 32]);
    }

    #[test]
    fn hmac_key_persisted_across_reopens() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");

        let key1 = {
            let db = Database::open(&path).unwrap();
            db.audit_hmac_key
        };
        let key2 = {
            let db = Database::open(&path).unwrap();
            db.audit_hmac_key
        };
        assert_eq!(key1, key2, "HMAC key must be stable across reopens");
    }

    #[test]
    fn hmac_key_unique_per_database() {
        let dir = tempfile::TempDir::new().unwrap();
        let db1 = Database::open(&dir.path().join("a.db")).unwrap();
        let db2 = Database::open(&dir.path().join("b.db")).unwrap();
        assert_ne!(
            db1.audit_hmac_key, db2.audit_hmac_key,
            "each database must have a unique HMAC key"
        );
    }

    #[test]
    fn audit_hash_uses_hmac_not_plain_sha256() {
        use sha2::{Digest, Sha256};

        let db = Database::open_in_memory().unwrap();
        let event = DomainEvent::new(EventPayload::SessionStarted {
            session_id: uuid::Uuid::new_v4(),
        });
        db.append_audit_event(&event).unwrap();

        let conn = db.conn.lock().unwrap();
        let (prev, stored_hash): (String, String) = conn
            .query_row(
                "SELECT previous_hash, hash FROM audit_log ORDER BY id LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        // Verify the stored hash is NOT the plain SHA-256 of the inputs.
        let payload_json = serde_json::to_string(&event.payload).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(prev.as_bytes());
        hasher.update(event.id.to_string().as_bytes());
        hasher.update(event.timestamp.to_rfc3339().as_bytes());
        hasher.update(payload_json.as_bytes());
        let plain_sha256 = hex::encode(hasher.finalize());

        assert_ne!(
            stored_hash, plain_sha256,
            "audit hash must be HMAC-SHA256, not bare SHA-256"
        );
        // And it must be a valid 64-char hex string (256 bits).
        assert_eq!(stored_hash.len(), 64);
        assert!(stored_hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn audit_hmac_key_table_exists_after_migration() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn.lock().unwrap();
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='audit_hmac_key'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "audit_hmac_key table must exist after migrations");
    }
