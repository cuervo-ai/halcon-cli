use rusqlite::Connection;

/// List of migrations in order. Each is (version, name, sql).
const MIGRATIONS: &[(u32, &str, &str)] = &[
    (1, "initial_schema", MIGRATION_001),
    (2, "trace_steps", MIGRATION_002),
    (3, "memory_system", MIGRATION_003),
    (4, "response_cache", MIGRATION_004),
    (5, "invocation_metrics", MIGRATION_005),
    (6, "resilience_events", MIGRATION_006),
    (7, "session_metrics_columns", MIGRATION_007),
    (8, "planning_steps", MIGRATION_008),
    (9, "policy_decisions", MIGRATION_009),
    (10, "episodic_memory", MIGRATION_010),
    (11, "tool_metrics_and_audit_session", MIGRATION_011),
    (12, "agent_tasks", MIGRATION_012),
    (13, "replay_support", MIGRATION_013),
    (14, "agent_state_checkpoint", MIGRATION_014),
    (15, "metrics_composite_index", MIGRATION_015),
    (16, "structured_tasks", MIGRATION_016),
    (17, "reasoning_experience", MIGRATION_017),
];

const MIGRATION_001: &str = r#"
-- Sessions table
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    title TEXT,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    working_directory TEXT NOT NULL,
    messages_json TEXT NOT NULL DEFAULT '[]',
    total_input_tokens INTEGER NOT NULL DEFAULT 0,
    total_output_tokens INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at DESC);

-- Audit log with hash chain (immutable, SOC 2 compliant)
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    timestamp TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    previous_hash TEXT NOT NULL,
    hash TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_log_event_type ON audit_log(event_type);

-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at TEXT NOT NULL
);
"#;

const MIGRATION_002: &str = r#"
-- Trace steps: append-only log of agent loop execution for deterministic replay.
CREATE TABLE IF NOT EXISTS trace_steps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    step_type TEXT NOT NULL,
    data_json TEXT NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    timestamp TEXT NOT NULL,
    UNIQUE(session_id, step_index)
);

CREATE INDEX IF NOT EXISTS idx_trace_steps_session ON trace_steps(session_id, step_index);
"#;

const MIGRATION_003: &str = r#"
-- Persistent semantic memory: facts, summaries, decisions, code, metadata.
CREATE TABLE IF NOT EXISTS memory_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entry_id TEXT NOT NULL UNIQUE,
    session_id TEXT,
    entry_type TEXT NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    embedding BLOB,
    embedding_model TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT,
    relevance_score REAL NOT NULL DEFAULT 1.0
);

CREATE INDEX IF NOT EXISTS idx_memory_type ON memory_entries(entry_type);
CREATE INDEX IF NOT EXISTS idx_memory_session ON memory_entries(session_id);
CREATE INDEX IF NOT EXISTS idx_memory_created ON memory_entries(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_memory_hash ON memory_entries(content_hash);

-- FTS5 virtual table for BM25 text search (auto-synced via triggers).
CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
    content,
    content=memory_entries,
    content_rowid=id,
    tokenize='porter unicode61'
);

-- Triggers to keep FTS index in sync with memory_entries.
CREATE TRIGGER memory_fts_ai AFTER INSERT ON memory_entries BEGIN
    INSERT INTO memory_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER memory_fts_ad AFTER DELETE ON memory_entries BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER memory_fts_au AFTER UPDATE ON memory_entries BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
    INSERT INTO memory_fts(rowid, content) VALUES (new.id, new.content);
END;
"#;

const MIGRATION_004: &str = r#"
-- Response cache: stores model responses keyed by semantic hash.
CREATE TABLE IF NOT EXISTS response_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cache_key TEXT NOT NULL UNIQUE,
    model TEXT NOT NULL,
    response_text TEXT NOT NULL,
    tool_calls_json TEXT,
    stop_reason TEXT NOT NULL,
    usage_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,
    hit_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_cache_key ON response_cache(cache_key);
CREATE INDEX IF NOT EXISTS idx_cache_expires ON response_cache(expires_at);
"#;

const MIGRATION_005: &str = r#"
-- Invocation metrics: per-request cost, latency, token tracking.
CREATE TABLE IF NOT EXISTS invocation_metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    latency_ms INTEGER NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    estimated_cost_usd REAL NOT NULL DEFAULT 0.0,
    success INTEGER NOT NULL DEFAULT 1,
    stop_reason TEXT NOT NULL DEFAULT 'unknown',
    session_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_metrics_model ON invocation_metrics(provider, model);
CREATE INDEX IF NOT EXISTS idx_metrics_created ON invocation_metrics(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_metrics_session ON invocation_metrics(session_id);
"#;

const MIGRATION_006: &str = r#"
-- Resilience events: circuit breaker trips, health changes, saturation, fallbacks.
CREATE TABLE IF NOT EXISTS resilience_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    event_type TEXT NOT NULL,
    from_state TEXT,
    to_state TEXT,
    score INTEGER,
    details TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_resilience_provider ON resilience_events(provider);
CREATE INDEX IF NOT EXISTS idx_resilience_type ON resilience_events(event_type);
CREATE INDEX IF NOT EXISTS idx_resilience_created ON resilience_events(created_at DESC);
"#;

const MIGRATION_007: &str = r#"
-- Session metrics: persist tool_invocations, agent_rounds, latency, cost.
ALTER TABLE sessions ADD COLUMN tool_invocations INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN agent_rounds INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN total_latency_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN estimated_cost_usd REAL NOT NULL DEFAULT 0.0;
"#;

const MIGRATION_008: &str = r#"
-- Planning steps: execution plan tracking with outcomes.
CREATE TABLE IF NOT EXISTS planning_steps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id TEXT NOT NULL,
    parent_plan_id TEXT,
    session_id TEXT NOT NULL,
    goal TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    description TEXT NOT NULL,
    tool_name TEXT,
    confidence REAL NOT NULL DEFAULT 0.0,
    outcome TEXT,
    outcome_detail TEXT,
    replan_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_planning_session ON planning_steps(session_id);
CREATE INDEX IF NOT EXISTS idx_planning_plan ON planning_steps(plan_id);
CREATE INDEX IF NOT EXISTS idx_planning_created ON planning_steps(created_at DESC);
"#;

const MIGRATION_009: &str = r#"
-- TBAC policy decision audit trail.
CREATE TABLE IF NOT EXISTS policy_decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    context_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    decision TEXT NOT NULL,
    reason TEXT,
    arguments_hash TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_policy_session ON policy_decisions(session_id);
CREATE INDEX IF NOT EXISTS idx_policy_context ON policy_decisions(context_id);
CREATE INDEX IF NOT EXISTS idx_policy_created ON policy_decisions(created_at DESC);
"#;

const MIGRATION_010: &str = r#"
-- Episodic memory: group entries into task-scoped episodes.
CREATE TABLE IF NOT EXISTS memory_episodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    episode_id TEXT NOT NULL UNIQUE,
    session_id TEXT,
    title TEXT NOT NULL,
    summary TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_episode_session ON memory_episodes(session_id);
CREATE INDEX IF NOT EXISTS idx_episode_started ON memory_episodes(started_at DESC);

-- Join table: many-to-many between entries and episodes.
CREATE TABLE IF NOT EXISTS memory_entry_episodes (
    entry_id INTEGER NOT NULL REFERENCES memory_entries(id) ON DELETE CASCADE,
    episode_id INTEGER NOT NULL REFERENCES memory_episodes(id) ON DELETE CASCADE,
    position INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (entry_id, episode_id)
);

CREATE INDEX IF NOT EXISTS idx_entry_episode ON memory_entry_episodes(episode_id);
"#;

const MIGRATION_011: &str = r#"
-- Tool execution metrics for tracking per-tool performance.
CREATE TABLE IF NOT EXISTS tool_execution_metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tool_name TEXT NOT NULL,
    session_id TEXT,
    duration_ms INTEGER NOT NULL,
    success INTEGER NOT NULL DEFAULT 1,
    is_parallel INTEGER NOT NULL DEFAULT 0,
    input_summary TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tool_metrics_name ON tool_execution_metrics(tool_name);
CREATE INDEX IF NOT EXISTS idx_tool_metrics_session ON tool_execution_metrics(session_id);
CREATE INDEX IF NOT EXISTS idx_tool_metrics_created ON tool_execution_metrics(created_at);

-- Add session_id column to audit_log for efficient session queries.
ALTER TABLE audit_log ADD COLUMN session_id TEXT;
CREATE INDEX IF NOT EXISTS idx_audit_session ON audit_log(session_id);
"#;

const MIGRATION_012: &str = r#"
-- Agent task tracking for multi-agent orchestrator.
CREATE TABLE IF NOT EXISTS agent_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL UNIQUE,
    orchestrator_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    agent_type TEXT NOT NULL,
    instruction TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0.0,
    latency_ms INTEGER NOT NULL DEFAULT 0,
    rounds INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    output_text TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_agent_tasks_orchestrator ON agent_tasks(orchestrator_id);
CREATE INDEX IF NOT EXISTS idx_agent_tasks_session ON agent_tasks(session_id);
CREATE INDEX IF NOT EXISTS idx_agent_tasks_status ON agent_tasks(status);
"#;

const MIGRATION_013: &str = r#"
-- Session checkpoints for resume-from-checkpoint replay.
CREATE TABLE IF NOT EXISTS session_checkpoints (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    round INTEGER NOT NULL,
    step_index INTEGER NOT NULL,
    messages_json TEXT NOT NULL,
    usage_json TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(session_id, round)
);

CREATE INDEX IF NOT EXISTS idx_checkpoint_session ON session_checkpoints(session_id);

-- Add execution fingerprint and replay metadata to sessions.
ALTER TABLE sessions ADD COLUMN execution_fingerprint TEXT;
ALTER TABLE sessions ADD COLUMN replay_source_session TEXT;
"#;

const MIGRATION_014: &str = r#"
-- Add agent_state to session checkpoints for state machine tracking.
ALTER TABLE session_checkpoints ADD COLUMN agent_state TEXT;
"#;

const MIGRATION_015: &str = r#"
-- Composite index for provider_metrics_windowed() queries.
-- Covers: WHERE provider = ? AND created_at >= ? ORDER BY latency_ms
CREATE INDEX IF NOT EXISTS idx_metrics_provider_created ON invocation_metrics(provider, created_at DESC);
"#;

const MIGRATION_016: &str = r#"
-- Structured tasks: formal task framework with provenance and artifact tracking.
CREATE TABLE IF NOT EXISTS structured_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL UNIQUE,
    session_id TEXT,
    plan_id TEXT,
    step_index INTEGER,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'Pending',
    priority INTEGER NOT NULL DEFAULT 0,
    depends_on_json TEXT NOT NULL DEFAULT '[]',
    inputs_json TEXT NOT NULL DEFAULT '{}',
    outputs_json TEXT NOT NULL DEFAULT '{}',
    artifacts_json TEXT NOT NULL DEFAULT '[]',
    provenance_json TEXT,
    retry_policy_json TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0,
    tags_json TEXT NOT NULL DEFAULT '[]',
    tool_name TEXT,
    expected_args_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    duration_ms INTEGER
);

CREATE INDEX IF NOT EXISTS idx_structured_tasks_session ON structured_tasks(session_id);
CREATE INDEX IF NOT EXISTS idx_structured_tasks_status ON structured_tasks(status);
CREATE INDEX IF NOT EXISTS idx_structured_tasks_plan ON structured_tasks(plan_id);
"#;

const MIGRATION_017: &str = r#"
-- Reasoning experience: UCB1 multi-armed bandit learning.
-- Stores average score and usage count per (task_type, strategy) pair.
CREATE TABLE IF NOT EXISTS reasoning_experience (
    task_type TEXT NOT NULL,
    strategy TEXT NOT NULL,
    avg_score REAL NOT NULL DEFAULT 0.0,
    uses INTEGER NOT NULL DEFAULT 0,
    last_score REAL,
    last_updated INTEGER NOT NULL,
    PRIMARY KEY (task_type, strategy)
);

CREATE INDEX IF NOT EXISTS idx_reasoning_exp_updated ON reasoning_experience(last_updated DESC);
"#;

/// Run all pending migrations.
pub fn run_migrations(conn: &Connection) -> Result<(), cuervo_core::error::CuervoError> {
    // Ensure migrations table exists
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )
    .map_err(|e| cuervo_core::error::CuervoError::MigrationError(e.to_string()))?;

    let current_version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| cuervo_core::error::CuervoError::MigrationError(e.to_string()))?;

    for &(version, name, sql) in MIGRATIONS {
        if version > current_version {
            tracing::info!("Running migration {version}: {name}");
            conn.execute_batch(sql).map_err(|e| {
                cuervo_core::error::CuervoError::MigrationError(format!(
                    "migration {version} ({name}): {e}"
                ))
            })?;

            conn.execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![version, name, chrono::Utc::now().to_rfc3339()],
            )
            .map_err(|e| cuervo_core::error::CuervoError::MigrationError(e.to_string()))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_run_on_fresh_db() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 17);
    }

    #[test]
    fn migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // Should not fail

        let count: u32 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 17);
    }

    #[test]
    fn migration_003_creates_memory_tables() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify memory_entries table exists
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memory_entries'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "memory_entries table should exist");

        // Verify FTS5 virtual table exists
        let fts_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memory_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(fts_exists, "memory_fts virtual table should exist");

        // Verify triggers exist
        let trigger_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'memory_fts_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trigger_count, 3, "should have 3 FTS sync triggers");
    }

    #[test]
    fn migration_003_fts_sync_works() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Insert a memory entry
        conn.execute(
            "INSERT INTO memory_entries (entry_id, entry_type, content, content_hash, created_at)
             VALUES ('test-1', 'fact', 'Rust workspace with nine crates', 'hash1', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        // FTS should find it
        let found: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM memory_fts WHERE memory_fts MATCH 'rust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(found, "FTS should find 'rust' after insert");

        // Porter stemming: "crates" should match "crate"
        let stemmed: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM memory_fts WHERE memory_fts MATCH 'crate'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stemmed, "Porter stemming should match 'crate' to 'crates'");

        // Delete the entry
        conn.execute("DELETE FROM memory_entries WHERE entry_id = 'test-1'", [])
            .unwrap();

        // FTS should no longer find it
        let gone: bool = conn
            .query_row(
                "SELECT COUNT(*) = 0 FROM memory_fts WHERE memory_fts MATCH 'rust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(gone, "FTS should not find 'rust' after delete");
    }

    #[test]
    fn migration_004_creates_cache_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='response_cache'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "response_cache table should exist");

        // Verify indexes exist
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_cache_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 2, "should have 2 cache indexes");
    }

    #[test]
    fn migration_005_creates_metrics_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='invocation_metrics'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "invocation_metrics table should exist");

        // Verify indexes exist
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_metrics_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 4, "should have 4 metrics indexes (3 original + 1 composite)");
    }

    #[test]
    fn migration_007_adds_session_metrics_columns() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify the new columns exist by inserting with them.
        conn.execute(
            "INSERT INTO sessions (id, model, provider, working_directory, messages_json, created_at, updated_at, tool_invocations, agent_rounds, total_latency_ms, estimated_cost_usd)
             VALUES ('test-id', 'echo', 'echo', '/tmp', '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 5, 3, 1500, 0.042)",
            [],
        )
        .unwrap();

        let (ti, ar, lat, cost): (u32, u32, i64, f64) = conn
            .query_row(
                "SELECT tool_invocations, agent_rounds, total_latency_ms, estimated_cost_usd FROM sessions WHERE id = 'test-id'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(ti, 5);
        assert_eq!(ar, 3);
        assert_eq!(lat, 1500);
        assert!((cost - 0.042).abs() < 0.001);
    }

    #[test]
    fn migration_008_creates_planning_steps_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='planning_steps'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "planning_steps table should exist");

        // Verify indexes exist
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_planning_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 3, "should have 3 planning indexes");

        // Insert a planning step
        conn.execute(
            "INSERT INTO planning_steps (plan_id, session_id, goal, step_index, description, tool_name, confidence)
             VALUES ('plan-1', 'sess-1', 'Fix bug', 0, 'Read the file', 'read_file', 0.9)",
            [],
        )
        .unwrap();

        let (goal, desc): (String, String) = conn
            .query_row(
                "SELECT goal, description FROM planning_steps WHERE plan_id = 'plan-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(goal, "Fix bug");
        assert_eq!(desc, "Read the file");
    }

    #[test]
    fn migration_009_creates_policy_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='policy_decisions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "policy_decisions table should exist");

        // Verify indexes exist
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_policy_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 3, "should have 3 policy indexes");

        // Insert a policy decision
        conn.execute(
            "INSERT INTO policy_decisions (session_id, context_id, tool_name, decision, reason)
             VALUES ('sess-1', 'ctx-1', 'bash', 'allowed', 'tool in allowlist')",
            [],
        )
        .unwrap();

        let (tool, decision): (String, String) = conn
            .query_row(
                "SELECT tool_name, decision FROM policy_decisions WHERE context_id = 'ctx-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tool, "bash");
        assert_eq!(decision, "allowed");
    }

    #[test]
    fn migration_006_creates_resilience_events_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='resilience_events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "resilience_events table should exist");

        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_resilience_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 3, "should have 3 resilience indexes");
    }

    #[test]
    fn migration_010_creates_episode_tables() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify memory_episodes table exists.
        let episodes_exist: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memory_episodes'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(episodes_exist, "memory_episodes table should exist");

        // Verify memory_entry_episodes join table exists.
        let join_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memory_entry_episodes'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(join_exists, "memory_entry_episodes table should exist");

        // Verify indexes exist.
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_episode_%' OR name = 'idx_entry_episode'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(idx_count >= 3, "should have at least 3 episode indexes, got {idx_count}");
    }

    #[test]
    fn migration_011_creates_tool_metrics_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify tool_execution_metrics table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='tool_execution_metrics'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "tool_execution_metrics table should exist");

        // Verify indexes exist.
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_tool_metrics_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 3, "should have 3 tool_metrics indexes");

        // Verify audit_log session_id column exists.
        let has_session_col: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('audit_log') WHERE name = 'session_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_session_col, "audit_log should have session_id column");

        // Verify audit session_id index.
        let audit_idx: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name = 'idx_audit_session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(audit_idx, "idx_audit_session index should exist");
    }

    #[test]
    fn migration_012_creates_agent_tasks_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify agent_tasks table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='agent_tasks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "agent_tasks table should exist");

        // Verify indexes exist.
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_agent_tasks_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 3, "should have 3 agent_tasks indexes");

        // Insert a task record.
        conn.execute(
            "INSERT INTO agent_tasks (task_id, orchestrator_id, session_id, agent_type, instruction, status, created_at)
             VALUES ('t-1', 'o-1', 's-1', 'Chat', 'Do work', 'running', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let (inst, status): (String, String) = conn
            .query_row(
                "SELECT instruction, status FROM agent_tasks WHERE task_id = 't-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(inst, "Do work");
        assert_eq!(status, "running");
    }

    #[test]
    fn migration_013_creates_checkpoints_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify session_checkpoints table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='session_checkpoints'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "session_checkpoints table should exist");

        // Verify index exists.
        let idx_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_checkpoint_session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(idx_exists, "idx_checkpoint_session index should exist");

        // Verify new session columns exist.
        let has_fingerprint: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('sessions') WHERE name = 'execution_fingerprint'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_fingerprint, "sessions should have execution_fingerprint column");

        let has_replay_source: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('sessions') WHERE name = 'replay_source_session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_replay_source, "sessions should have replay_source_session column");

        // Insert a checkpoint.
        conn.execute(
            "INSERT INTO session_checkpoints (session_id, round, step_index, messages_json, usage_json, fingerprint, created_at)
             VALUES ('s-1', 0, 5, '[]', '{}', 'abc123', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let (round, fp): (u32, String) = conn
            .query_row(
                "SELECT round, fingerprint FROM session_checkpoints WHERE session_id = 's-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(round, 0);
        assert_eq!(fp, "abc123");
    }

    #[test]
    fn migration_014_adds_agent_state_column() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Insert checkpoint with agent_state.
        conn.execute(
            "INSERT INTO session_checkpoints (session_id, round, step_index, messages_json, usage_json, fingerprint, created_at, agent_state)
             VALUES ('s-2', 0, 0, '[]', '{}', 'fp1', '2026-01-01T00:00:00Z', 'executing')",
            [],
        )
        .unwrap();

        let state: Option<String> = conn
            .query_row(
                "SELECT agent_state FROM session_checkpoints WHERE session_id = 's-2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state.as_deref(), Some("executing"));

        // NULL agent_state should also work.
        conn.execute(
            "INSERT INTO session_checkpoints (session_id, round, step_index, messages_json, usage_json, fingerprint, created_at)
             VALUES ('s-3', 0, 0, '[]', '{}', 'fp2', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let state2: Option<String> = conn
            .query_row(
                "SELECT agent_state FROM session_checkpoints WHERE session_id = 's-3'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(state2.is_none());
    }

    #[test]
    fn migration_016_creates_structured_tasks_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify structured_tasks table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='structured_tasks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "structured_tasks table should exist");

        // Verify indexes exist.
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_structured_tasks_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 3, "should have 3 structured_tasks indexes");

        // Insert a task record.
        conn.execute(
            "INSERT INTO structured_tasks (task_id, title, description, status, priority, depends_on_json, retry_policy_json, created_at)
             VALUES ('t-1', 'Read file', 'Read the config', 'Ready', 10, '[]', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let (title, status): (String, String) = conn
            .query_row(
                "SELECT title, status FROM structured_tasks WHERE task_id = 't-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "Read file");
        assert_eq!(status, "Ready");
    }
}
