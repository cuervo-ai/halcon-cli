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
    (18, "permission_rules", MIGRATION_018),
    (19, "sdlc_context_servers", MIGRATION_019),
    (20, "native_search_engine", MIGRATION_020),
    (21, "activity_search_history", MIGRATION_021),
    (22, "search_embeddings", MIGRATION_022),
    (23, "search_feedback_loop", MIGRATION_023),
    (24, "observability", MIGRATION_024),
    (25, "metrics_snapshots", MIGRATION_025),
    (26, "session_messages_compression", MIGRATION_026),
    (27, "media_cache", MIGRATION_027),
    (28, "media_index", MIGRATION_028),
    (29, "palette_optimization_history", MIGRATION_029),
    (30, "model_quality_stats", MIGRATION_030),
    (31, "plugin_system", MIGRATION_031),
    (32, "audit_hmac_key", MIGRATION_032),
    (33, "media_index_description", MIGRATION_033),
    (34, "plugin_circuit_state", MIGRATION_034),
    (35, "execution_loop_events", MIGRATION_035),
    (36, "daily_user_metrics", MIGRATION_036),
    (37, "mailbox_messages", MIGRATION_037),
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

const MIGRATION_018: &str = r#"
-- Permission rules: persistent authorization decisions with scoping and pattern matching.
-- Enables contextual permission system:
--   - Session: temporary rules (expired on exit)
--   - Directory: scoped to specific working directory
--   - Repository: scoped to git repository root
--   - Global: applies everywhere
CREATE TABLE IF NOT EXISTS permission_rules (
    rule_id TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK(scope IN ('session', 'directory', 'repository', 'global')),
    scope_value TEXT NOT NULL,
    tool_pattern TEXT NOT NULL,
    tool_pattern_type TEXT NOT NULL CHECK(tool_pattern_type IN ('exact', 'glob', 'regex')),
    param_pattern TEXT,
    decision TEXT NOT NULL CHECK(decision IN (
        'allowed', 'allowed_always', 'allowed_for_directory', 'allowed_for_repository',
        'allowed_for_pattern', 'allowed_this_session', 'denied', 'denied_for_directory',
        'denied_for_pattern'
    )),
    reason TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    expires_at TEXT,
    active INTEGER NOT NULL DEFAULT 1
);

-- Performance indexes for rule matching (O(1) exact → O(D) directory → O(P) pattern → O(1) global)
CREATE INDEX IF NOT EXISTS idx_permission_rules_lookup
    ON permission_rules(scope, scope_value, tool_pattern, active)
    WHERE active = 1;

CREATE INDEX IF NOT EXISTS idx_permission_rules_tool
    ON permission_rules(tool_pattern, active)
    WHERE active = 1;

CREATE INDEX IF NOT EXISTS idx_permission_rules_scope
    ON permission_rules(scope, active)
    WHERE active = 1;

CREATE INDEX IF NOT EXISTS idx_permission_rules_created
    ON permission_rules(created_at DESC);
"#;

const MIGRATION_019: &str = r#"
-- SDLC Context Servers: persistent state for phase-aware context delivery.
-- Enables SDLC-aware context system:
--   - Product requirements (Discovery phase)
--   - Architecture decisions (Planning phase)
--   - Code patterns & structure (Implementation phase)
--   - Test results & coverage (Testing phase)
--   - Runtime logs & metrics (Monitoring phase)
--   - Support tickets & incidents (Support phase)

-- Product requirements (Server 1: Requirements & Product)
CREATE TABLE IF NOT EXISTS product_requirements (
    req_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('proposed', 'approved', 'in_development', 'completed', 'cancelled')),
    priority INTEGER NOT NULL CHECK(priority >= 0 AND priority <= 3),
    dependencies_json TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- FTS5 index for semantic search over requirements
CREATE VIRTUAL TABLE IF NOT EXISTS product_requirements_fts USING fts5(
    title, description, content='product_requirements', content_rowid='rowid'
);

-- Triggers to keep FTS5 index synchronized
CREATE TRIGGER IF NOT EXISTS product_requirements_ai AFTER INSERT ON product_requirements BEGIN
    INSERT INTO product_requirements_fts(rowid, title, description)
    VALUES (new.rowid, new.title, new.description);
END;

CREATE TRIGGER IF NOT EXISTS product_requirements_au AFTER UPDATE ON product_requirements BEGIN
    UPDATE product_requirements_fts SET title = new.title, description = new.description
    WHERE rowid = new.rowid;
END;

CREATE TRIGGER IF NOT EXISTS product_requirements_ad AFTER DELETE ON product_requirements BEGIN
    DELETE FROM product_requirements_fts WHERE rowid = old.rowid;
END;

-- Architecture documents (Server 2: Architecture & Design)
CREATE TABLE IF NOT EXISTS architecture_documents (
    doc_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    doc_type TEXT NOT NULL CHECK(doc_type IN ('ADR', 'Design', 'Diagram', 'Specification', 'RFC')),
    status TEXT NOT NULL CHECK(status IN ('draft', 'active', 'approved', 'superseded', 'deprecated')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- FTS5 index for semantic search over architecture docs
CREATE VIRTUAL TABLE IF NOT EXISTS architecture_documents_fts USING fts5(
    title, content, content='architecture_documents', content_rowid='rowid'
);

-- Triggers to keep FTS5 index synchronized
CREATE TRIGGER IF NOT EXISTS architecture_documents_ai AFTER INSERT ON architecture_documents BEGIN
    INSERT INTO architecture_documents_fts(rowid, title, content)
    VALUES (new.rowid, new.title, new.content);
END;

CREATE TRIGGER IF NOT EXISTS architecture_documents_au AFTER UPDATE ON architecture_documents BEGIN
    UPDATE architecture_documents_fts SET title = new.title, content = new.content
    WHERE rowid = new.rowid;
END;

CREATE TRIGGER IF NOT EXISTS architecture_documents_ad AFTER DELETE ON architecture_documents BEGIN
    DELETE FROM architecture_documents_fts WHERE rowid = old.rowid;
END;

-- CI/CD Workflows (Server 4: Workflow & CI/CD)
CREATE TABLE IF NOT EXISTS ci_workflows (
    workflow_id TEXT PRIMARY KEY,
    workflow_name TEXT NOT NULL,
    workflow_file TEXT NOT NULL,
    description TEXT NOT NULL,
    trigger_events TEXT NOT NULL,
    last_run_status TEXT CHECK(last_run_status IN ('success', 'failure', 'pending', 'cancelled', 'skipped')),
    last_run_at INTEGER,
    last_run_duration_ms INTEGER,
    failure_count INTEGER NOT NULL DEFAULT 0,
    success_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- FTS5 index for semantic search over workflows
CREATE VIRTUAL TABLE IF NOT EXISTS ci_workflows_fts USING fts5(
    workflow_name, description, workflow_file, content='ci_workflows', content_rowid='rowid'
);

-- Triggers to keep FTS5 index synchronized
CREATE TRIGGER IF NOT EXISTS ci_workflows_ai AFTER INSERT ON ci_workflows BEGIN
    INSERT INTO ci_workflows_fts(rowid, workflow_name, description, workflow_file)
    VALUES (new.rowid, new.workflow_name, new.description, new.workflow_file);
END;

CREATE TRIGGER IF NOT EXISTS ci_workflows_au AFTER UPDATE ON ci_workflows BEGIN
    UPDATE ci_workflows_fts SET workflow_name = new.workflow_name, description = new.description, workflow_file = new.workflow_file
    WHERE rowid = new.rowid;
END;

CREATE TRIGGER IF NOT EXISTS ci_workflows_ad AFTER DELETE ON ci_workflows BEGIN
    DELETE FROM ci_workflows_fts WHERE rowid = old.rowid;
END;

-- Test Results (Server 5: Test Coverage & Results)
CREATE TABLE IF NOT EXISTS test_results (
    test_id TEXT PRIMARY KEY,
    test_suite TEXT NOT NULL,
    test_name TEXT NOT NULL,
    test_file TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('passed', 'failed', 'skipped', 'error')),
    duration_ms INTEGER,
    failure_message TEXT,
    stack_trace TEXT,
    coverage_percent REAL,
    assertions_count INTEGER,
    run_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

-- FTS5 index for semantic search over test results
CREATE VIRTUAL TABLE IF NOT EXISTS test_results_fts USING fts5(
    test_suite, test_name, test_file, failure_message, content='test_results', content_rowid='rowid'
);

-- Triggers to keep FTS5 index synchronized
CREATE TRIGGER IF NOT EXISTS test_results_ai AFTER INSERT ON test_results BEGIN
    INSERT INTO test_results_fts(rowid, test_suite, test_name, test_file, failure_message)
    VALUES (new.rowid, new.test_suite, new.test_name, new.test_file, COALESCE(new.failure_message, ''));
END;

CREATE TRIGGER IF NOT EXISTS test_results_au AFTER UPDATE ON test_results BEGIN
    UPDATE test_results_fts SET test_suite = new.test_suite, test_name = new.test_name, test_file = new.test_file, failure_message = COALESCE(new.failure_message, '')
    WHERE rowid = new.rowid;
END;

CREATE TRIGGER IF NOT EXISTS test_results_ad AFTER DELETE ON test_results BEGIN
    DELETE FROM test_results_fts WHERE rowid = old.rowid;
END;

-- Runtime Metrics (Server 6: Monitoring & Observability)
CREATE TABLE IF NOT EXISTS runtime_metrics (
    metric_id TEXT PRIMARY KEY,
    metric_name TEXT NOT NULL,
    metric_type TEXT NOT NULL CHECK(metric_type IN ('counter', 'gauge', 'histogram', 'summary')),
    metric_value REAL NOT NULL,
    labels_json TEXT NOT NULL DEFAULT '{}',
    service_name TEXT NOT NULL,
    environment TEXT CHECK(environment IN ('dev', 'staging', 'production')),
    severity TEXT CHECK(severity IN ('info', 'warning', 'error', 'critical')),
    message TEXT,
    timestamp INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

-- FTS5 index for semantic search over runtime metrics
CREATE VIRTUAL TABLE IF NOT EXISTS runtime_metrics_fts USING fts5(
    metric_name, service_name, message, content='runtime_metrics', content_rowid='rowid'
);

-- Triggers to keep FTS5 index synchronized
CREATE TRIGGER IF NOT EXISTS runtime_metrics_ai AFTER INSERT ON runtime_metrics BEGIN
    INSERT INTO runtime_metrics_fts(rowid, metric_name, service_name, message)
    VALUES (new.rowid, new.metric_name, new.service_name, COALESCE(new.message, ''));
END;

CREATE TRIGGER IF NOT EXISTS runtime_metrics_au AFTER UPDATE ON runtime_metrics BEGIN
    UPDATE runtime_metrics_fts SET metric_name = new.metric_name, service_name = new.service_name, message = COALESCE(new.message, '')
    WHERE rowid = new.rowid;
END;

CREATE TRIGGER IF NOT EXISTS runtime_metrics_ad AFTER DELETE ON runtime_metrics BEGIN
    DELETE FROM runtime_metrics_fts WHERE rowid = old.rowid;
END;

-- Security Findings (Server 7: Security & Compliance)
CREATE TABLE IF NOT EXISTS security_findings (
    finding_id TEXT PRIMARY KEY,
    finding_type TEXT NOT NULL CHECK(finding_type IN ('vulnerability', 'code_smell', 'secret_leak', 'compliance_violation', 'dependency_risk')),
    severity TEXT NOT NULL CHECK(severity IN ('critical', 'high', 'medium', 'low', 'info')),
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    affected_file TEXT,
    affected_line INTEGER,
    cve_id TEXT,
    cvss_score REAL,
    remediation TEXT,
    status TEXT NOT NULL CHECK(status IN ('open', 'acknowledged', 'in_progress', 'resolved', 'false_positive', 'wont_fix')),
    detected_at INTEGER NOT NULL,
    resolved_at INTEGER,
    created_at INTEGER NOT NULL
);

-- FTS5 index for semantic search over security findings
CREATE VIRTUAL TABLE IF NOT EXISTS security_findings_fts USING fts5(
    title, description, affected_file, remediation, content='security_findings', content_rowid='rowid'
);

-- Triggers to keep FTS5 index synchronized
CREATE TRIGGER IF NOT EXISTS security_findings_ai AFTER INSERT ON security_findings BEGIN
    INSERT INTO security_findings_fts(rowid, title, description, affected_file, remediation)
    VALUES (new.rowid, new.title, new.description, COALESCE(new.affected_file, ''), COALESCE(new.remediation, ''));
END;

CREATE TRIGGER IF NOT EXISTS security_findings_au AFTER UPDATE ON security_findings BEGIN
    UPDATE security_findings_fts SET title = new.title, description = new.description, affected_file = COALESCE(new.affected_file, ''), remediation = COALESCE(new.remediation, '')
    WHERE rowid = new.rowid;
END;

CREATE TRIGGER IF NOT EXISTS security_findings_ad AFTER DELETE ON security_findings BEGIN
    DELETE FROM security_findings_fts WHERE rowid = old.rowid;
END;

-- Support Incidents (Server 8: Support & Incidents)
CREATE TABLE IF NOT EXISTS support_incidents (
    incident_id TEXT PRIMARY KEY,
    incident_type TEXT NOT NULL CHECK(incident_type IN ('bug_report', 'feature_request', 'user_feedback', 'crash_report', 'performance_issue', 'question')),
    priority TEXT NOT NULL CHECK(priority IN ('critical', 'high', 'medium', 'low')),
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    reporter TEXT,
    affected_component TEXT,
    reproducible BOOLEAN NOT NULL DEFAULT 0,
    reproduction_steps TEXT,
    error_message TEXT,
    stack_trace TEXT,
    resolution TEXT,
    status TEXT NOT NULL CHECK(status IN ('new', 'triaged', 'in_progress', 'resolved', 'closed', 'duplicate', 'wont_fix')),
    reported_at INTEGER NOT NULL,
    resolved_at INTEGER,
    created_at INTEGER NOT NULL
);

-- FTS5 index for semantic search over support incidents
CREATE VIRTUAL TABLE IF NOT EXISTS support_incidents_fts USING fts5(
    title, description, affected_component, error_message, resolution, content='support_incidents', content_rowid='rowid'
);

-- Triggers to keep FTS5 index synchronized
CREATE TRIGGER IF NOT EXISTS support_incidents_ai AFTER INSERT ON support_incidents BEGIN
    INSERT INTO support_incidents_fts(rowid, title, description, affected_component, error_message, resolution)
    VALUES (new.rowid, new.title, new.description, COALESCE(new.affected_component, ''), COALESCE(new.error_message, ''), COALESCE(new.resolution, ''));
END;

CREATE TRIGGER IF NOT EXISTS support_incidents_au AFTER UPDATE ON support_incidents BEGIN
    UPDATE support_incidents_fts SET title = new.title, description = new.description, affected_component = COALESCE(new.affected_component, ''), error_message = COALESCE(new.error_message, ''), resolution = COALESCE(new.resolution, '')
    WHERE rowid = new.rowid;
END;

CREATE TRIGGER IF NOT EXISTS support_incidents_ad AFTER DELETE ON support_incidents BEGIN
    DELETE FROM support_incidents_fts WHERE rowid = old.rowid;
END;

-- SDLC phase history: tracks phase transitions per session
CREATE TABLE IF NOT EXISTS sdlc_phase_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN (
        'discovery', 'planning', 'implementation', 'testing',
        'review', 'deployment', 'monitoring', 'support'
    )),
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    active_servers_json TEXT NOT NULL DEFAULT '[]',
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX IF NOT EXISTS idx_sdlc_history_session ON sdlc_phase_history(session_id, started_at DESC);

-- Context server health: tracks operational status
CREATE TABLE IF NOT EXISTS context_server_health (
    server_name TEXT PRIMARY KEY,
    server_type TEXT NOT NULL CHECK(server_type IN (
        'requirements', 'architecture', 'codebase', 'workflow',
        'testing', 'runtime', 'security', 'support'
    )),
    health_status TEXT NOT NULL CHECK(health_status IN ('healthy', 'degraded', 'unavailable')),
    last_success_at INTEGER NOT NULL,
    last_failure_at INTEGER,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    latency_ms INTEGER,
    error_rate REAL NOT NULL DEFAULT 0.0,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_context_server_health_status
    ON context_server_health(health_status, server_type);
"#;

const MIGRATION_020: &str = r#"
-- Native Search Engine: local crawling, indexing, and retrieval.
-- Eliminates dependency on external search APIs (Brave, Google).

-- Documents store with zstd compression
CREATE TABLE IF NOT EXISTS search_documents (
    id BLOB PRIMARY KEY,
    url TEXT NOT NULL UNIQUE,
    domain TEXT NOT NULL,
    title TEXT NOT NULL,
    text TEXT NOT NULL,
    html_compressed BLOB,
    indexed_at TEXT NOT NULL,
    last_crawled TEXT,
    pagerank REAL NOT NULL DEFAULT 0.0,
    freshness_score REAL NOT NULL DEFAULT 1.0,
    outlink_count INTEGER NOT NULL DEFAULT 0,
    language TEXT
);

CREATE INDEX IF NOT EXISTS idx_search_documents_domain ON search_documents(domain);
CREATE INDEX IF NOT EXISTS idx_search_documents_indexed ON search_documents(indexed_at DESC);
CREATE INDEX IF NOT EXISTS idx_search_documents_pagerank ON search_documents(pagerank DESC);
CREATE INDEX IF NOT EXISTS idx_search_documents_url ON search_documents(url);

-- FTS5 full-text search index
CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(
    title,
    text,
    content='search_documents',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

-- Triggers to keep FTS5 in sync
CREATE TRIGGER IF NOT EXISTS search_documents_ai AFTER INSERT ON search_documents BEGIN
    INSERT INTO search_fts(rowid, title, text) VALUES (new.rowid, new.title, new.text);
END;

CREATE TRIGGER IF NOT EXISTS search_documents_ad AFTER DELETE ON search_documents BEGIN
    DELETE FROM search_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER IF NOT EXISTS search_documents_au AFTER UPDATE ON search_documents BEGIN
    UPDATE search_fts SET title = new.title, text = new.text WHERE rowid = new.rowid;
END;

-- Metadata store (structured data from HTML)
CREATE TABLE IF NOT EXISTS search_metadata (
    doc_id BLOB PRIMARY KEY REFERENCES search_documents(id) ON DELETE CASCADE,
    description TEXT,
    author TEXT,
    published_at TEXT,
    modified_at TEXT,
    keywords TEXT,
    structured_data TEXT,
    og_image TEXT,
    canonical_url TEXT
);

-- Outlinks graph (for PageRank computation)
CREATE TABLE IF NOT EXISTS search_links (
    source_id BLOB NOT NULL REFERENCES search_documents(id) ON DELETE CASCADE,
    target_url TEXT NOT NULL,
    anchor_text TEXT,
    PRIMARY KEY (source_id, target_url)
);

CREATE INDEX IF NOT EXISTS idx_search_links_target ON search_links(target_url);

-- Crawl queue / URL frontier (priority queue)
CREATE TABLE IF NOT EXISTS search_crawl_queue (
    url TEXT PRIMARY KEY,
    depth INTEGER NOT NULL,
    priority INTEGER NOT NULL,
    discovered_at TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'crawled', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_search_crawl_priority ON search_crawl_queue(priority DESC, depth ASC);
CREATE INDEX IF NOT EXISTS idx_search_crawl_status ON search_crawl_queue(status);

-- Crawl history (deduplication + ETag caching)
CREATE TABLE IF NOT EXISTS search_crawl_history (
    url TEXT PRIMARY KEY,
    url_hash TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_crawled TEXT,
    crawl_count INTEGER NOT NULL DEFAULT 0,
    last_status INTEGER,
    etag TEXT,
    last_modified TEXT
);

CREATE INDEX IF NOT EXISTS idx_search_crawl_history_hash ON search_crawl_history(url_hash);
CREATE INDEX IF NOT EXISTS idx_search_crawl_history_last ON search_crawl_history(last_crawled DESC);

-- Result cache (LRU with TTL)
CREATE TABLE IF NOT EXISTS search_result_cache (
    query_hash TEXT PRIMARY KEY,
    query TEXT NOT NULL,
    results BLOB NOT NULL,
    created_at TEXT NOT NULL,
    hit_count INTEGER NOT NULL DEFAULT 0,
    expires_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_search_cache_created ON search_result_cache(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_search_cache_expires ON search_result_cache(expires_at);
"#;

const MIGRATION_021: &str = r#"
-- Activity Search History: stores search queries for TUI activity zone.
-- Used for Up/Down arrow navigation in search overlay.
-- Phase 3 SRCH-004: Search history persistence.
CREATE TABLE IF NOT EXISTS activity_search_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    query TEXT NOT NULL,
    search_mode TEXT NOT NULL CHECK(search_mode IN ('exact', 'fuzzy', 'regex')),
    match_count INTEGER NOT NULL DEFAULT 0,
    searched_at TEXT NOT NULL,
    session_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_activity_search_history_time ON activity_search_history(searched_at DESC);
CREATE INDEX IF NOT EXISTS idx_activity_search_history_session ON activity_search_history(session_id, searched_at DESC);

-- Limit table size to last 1000 queries (rolling window).
-- Trigger auto-deletes old entries when limit exceeded.
CREATE TRIGGER IF NOT EXISTS activity_search_history_cleanup AFTER INSERT ON activity_search_history BEGIN
    DELETE FROM activity_search_history WHERE id IN (
        SELECT id FROM activity_search_history ORDER BY searched_at DESC LIMIT -1 OFFSET 1000
    );
END;
"#;

const MIGRATION_022: &str = r#"
-- Add embedding column to search_documents for semantic search.
-- Phase 3, Task 3.1: Hybrid BM25 + Semantic Embeddings.
-- Stores serialized f32 vectors (384 dimensions for AllMiniLML6V2 model).
ALTER TABLE search_documents ADD COLUMN embedding BLOB;

-- Index for future embedding similarity searches (not used in Phase 3.1, but planned for future).
-- Note: SQLite doesn't have native vector similarity search, so this is just a placeholder.
-- Future: migrate to pgvector or use dedicated vector database.
"#;

const MIGRATION_023: &str = r#"
-- Confidence Feedback Loop tables.
-- Phase 3, Task 3.2: Track search quality metrics and adapt ranking weights.

-- User interactions with search results (clicks, dwell time).
CREATE TABLE IF NOT EXISTS search_interactions (
    id TEXT PRIMARY KEY,
    query TEXT NOT NULL,
    document_id BLOB NOT NULL,
    position INTEGER NOT NULL,
    clicked_at TEXT NOT NULL,
    dwell_time_secs REAL,
    session_id TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_search_interactions_query ON search_interactions(query);
CREATE INDEX IF NOT EXISTS idx_search_interactions_session ON search_interactions(session_id);
CREATE INDEX IF NOT EXISTS idx_search_interactions_clicked_at ON search_interactions(clicked_at DESC);

-- Computed quality metrics per query.
CREATE TABLE IF NOT EXISTS query_quality_metrics (
    query TEXT PRIMARY KEY,
    execution_count INTEGER NOT NULL DEFAULT 0,
    ctr REAL NOT NULL DEFAULT 0.0,
    mrr REAL NOT NULL DEFAULT 0.0,
    avg_dwell_time REAL NOT NULL DEFAULT 0.0,
    abandonment_rate REAL NOT NULL DEFAULT 0.0,
    computed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_query_quality_metrics_computed_at ON query_quality_metrics(computed_at DESC);

-- Weight optimization history.
CREATE TABLE IF NOT EXISTS weight_optimization_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bm25_weight REAL NOT NULL,
    semantic_weight REAL NOT NULL,
    pagerank_weight REAL NOT NULL,
    avg_quality REAL NOT NULL,
    sample_size INTEGER NOT NULL,
    timestamp TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_weight_optimization_timestamp ON weight_optimization_history(timestamp DESC);
"#;

const MIGRATION_024: &str = r#"
-- Observability tables for search engine monitoring.
-- Phase 4, Task 4.1: Query instrumentation, regression detection, and performance tracking.

-- Query instrumentation (per-query timing and quality metrics).
CREATE TABLE IF NOT EXISTS query_instrumentation (
    query_id TEXT PRIMARY KEY,
    query TEXT NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    duration_ms INTEGER,
    result_count INTEGER NOT NULL DEFAULT 0,
    quality_score REAL,
    context_precision REAL,
    context_recall REAL,
    ndcg_at_10 REAL,
    error TEXT
);

CREATE INDEX IF NOT EXISTS idx_query_instrumentation_started_at ON query_instrumentation(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_query_instrumentation_query ON query_instrumentation(query);

-- Query phases (per-phase timing breakdown).
CREATE TABLE IF NOT EXISTS query_phases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    query_id TEXT NOT NULL,
    phase TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    timestamp TEXT NOT NULL,
    FOREIGN KEY (query_id) REFERENCES query_instrumentation(query_id)
);

CREATE INDEX IF NOT EXISTS idx_query_phases_query_id ON query_phases(query_id);

-- Regression alerts (automated quality degradation detection).
CREATE TABLE IF NOT EXISTS regression_alerts (
    alert_id TEXT PRIMARY KEY,
    regression_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    baseline_value REAL NOT NULL,
    current_value REAL NOT NULL,
    drop_percent REAL NOT NULL,
    triggered_at TEXT NOT NULL,
    message TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_regression_alerts_triggered_at ON regression_alerts(triggered_at DESC);
CREATE INDEX IF NOT EXISTS idx_regression_alerts_severity ON regression_alerts(severity);
"#;

const MIGRATION_025: &str = r#"
-- Historical metrics snapshots for long-term analysis.
-- Phase 4, Task 4.2: Time-series snapshot storage and trend caching.

CREATE TABLE IF NOT EXISTS metrics_snapshots (
    snapshot_id TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL,
    snapshot_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON metrics_snapshots(timestamp DESC);
"#;

const MIGRATION_026: &str = r#"
-- Add compressed messages column (zstd BLOB).
-- NULL = old row using messages_json column.
-- Non-null = new row with zstd-compressed messages (messages_json stores '[]' placeholder).
ALTER TABLE sessions ADD COLUMN messages_compressed BLOB;
"#;

const MIGRATION_027: &str = r#"
-- M27: media_cache — content-addressed analysis result cache
CREATE TABLE IF NOT EXISTS media_cache (
    content_hash   TEXT    PRIMARY KEY,
    modality       TEXT    NOT NULL CHECK (modality IN ('image','audio','video')),
    analysis_json  TEXT    NOT NULL,
    tile_count     INTEGER NOT NULL DEFAULT 1,
    token_estimate INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT    NOT NULL,
    accessed_at    TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_media_cache_accessed ON media_cache(accessed_at);
CREATE INDEX IF NOT EXISTS idx_media_cache_modality ON media_cache(modality);
"#;

const MIGRATION_028: &str = r#"
-- M28: media_index — CLIP embedding store for cross-modal retrieval
CREATE TABLE IF NOT EXISTS media_index (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    content_hash   TEXT    NOT NULL,
    modality       TEXT    NOT NULL CHECK (modality IN ('image','audio','video')),
    embedding_data BLOB    NOT NULL,
    embedding_dim  INTEGER NOT NULL,
    clip_start_secs REAL,
    clip_end_secs   REAL,
    session_id     TEXT,
    source_path    TEXT,
    created_at     TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_media_index_hash     ON media_index(content_hash);
CREATE INDEX IF NOT EXISTS idx_media_index_modality ON media_index(modality);
CREATE INDEX IF NOT EXISTS idx_media_index_session  ON media_index(session_id);
"#;

const MIGRATION_031: &str = r#"
-- M31: plugin_system — V3 plugin registry persistence.
-- installed_plugins stores manifest metadata for each registered plugin.
-- plugin_metrics stores per-plugin call counts and UCB1 reward signals for
-- cross-session bandit learning.
CREATE TABLE IF NOT EXISTS installed_plugins (
    plugin_id     TEXT NOT NULL,
    name          TEXT NOT NULL,
    version       TEXT NOT NULL,
    category      TEXT NOT NULL DEFAULT 'custom',
    manifest_toml TEXT NOT NULL DEFAULT '',
    installed_at  TEXT NOT NULL DEFAULT (datetime('now')),
    trust_level   TEXT NOT NULL DEFAULT 'local',
    PRIMARY KEY (plugin_id)
);
CREATE TABLE IF NOT EXISTS plugin_metrics (
    plugin_id        TEXT    NOT NULL,
    calls_made       INTEGER NOT NULL DEFAULT 0,
    calls_failed     INTEGER NOT NULL DEFAULT 0,
    tokens_used      INTEGER NOT NULL DEFAULT 0,
    ucb1_n_uses      INTEGER NOT NULL DEFAULT 0,
    ucb1_sum_rewards REAL    NOT NULL DEFAULT 0.0,
    updated_at       TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (plugin_id)
);
"#;

const MIGRATION_030: &str = r#"
-- M30: model_quality_stats — cross-session ModelPerformanceTracker persistence.
-- Stores per-model (success_count, failure_count, total_reward) so the ModelSelector
-- balanced routing strategy learns which models perform well across sessions.
CREATE TABLE IF NOT EXISTS model_quality_stats (
    model_id      TEXT    NOT NULL,
    provider      TEXT    NOT NULL,
    success_count INTEGER NOT NULL DEFAULT 0,
    failure_count INTEGER NOT NULL DEFAULT 0,
    total_reward  REAL    NOT NULL DEFAULT 0.0,
    updated_at    INTEGER NOT NULL,
    PRIMARY KEY (model_id, provider)
);
CREATE INDEX IF NOT EXISTS idx_model_quality_provider ON model_quality_stats(provider, updated_at DESC);
"#;

const MIGRATION_029: &str = r#"
-- M29: palette_optimization_history — cross-session warm-start for adaptive optimizer.
-- Append-only: each optimization run is recorded for future sessions to warm-start from.
CREATE TABLE IF NOT EXISTS palette_optimization_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id          TEXT    NOT NULL,
    base_hue            REAL    NOT NULL,
    initial_quality     REAL    NOT NULL,
    final_quality       REAL    NOT NULL,
    quality_delta       REAL    NOT NULL,
    iterations          INTEGER NOT NULL,
    convergence_status  TEXT    NOT NULL,
    duration_ms         INTEGER NOT NULL,
    steps_json          TEXT    NOT NULL DEFAULT '[]',
    created_at          TEXT    NOT NULL
);
-- Index for warm-start queries: hue bucket lookup (most recent first, best quality first).
CREATE INDEX IF NOT EXISTS idx_pal_opt_hue ON palette_optimization_history(base_hue, final_quality DESC);
CREATE INDEX IF NOT EXISTS idx_pal_opt_session ON palette_optimization_history(session_id, created_at DESC);
"#;

const MIGRATION_032: &str = r#"
-- M32: audit_hmac_key — per-database HMAC-SHA256 signing key for audit chain integrity.
-- Stores a single 256-bit key (hex-encoded) used to sign each audit log entry.
-- Without the key, an adversary who compromises the database cannot recompute valid hashes
-- to cover tampered entries (unlike bare SHA-256 which only requires the previous hash).
-- The key is generated on first open and never leaves the database host.
CREATE TABLE IF NOT EXISTS audit_hmac_key (
    key_id   INTEGER PRIMARY KEY CHECK(key_id = 1),
    key_hex  TEXT    NOT NULL,
    created_at TEXT  NOT NULL
);
"#;

const MIGRATION_033: &str = r#"
-- M33: media_index_description — add human-readable description to media_index.
-- Existing rows receive NULL (no re-analysis required).
-- New entries store the analysis description so MediaContextSource.gather() can
-- return useful chunk content without a separate cache lookup.
ALTER TABLE media_index ADD COLUMN description TEXT;
"#;

const MIGRATION_034: &str = r#"
-- M34: plugin_circuit_state — persist plugin circuit breaker state across sessions.
-- Plugins with historical failures restart in 'degraded' state (not 'clean'), preventing
-- repeated invocations of broken plugins across cold restarts.
-- state TEXT: clean | degraded | suspended | failed
CREATE TABLE IF NOT EXISTS plugin_circuit_state (
    plugin_id TEXT PRIMARY KEY,
    state TEXT NOT NULL DEFAULT 'clean' CHECK(state IN ('clean', 'degraded', 'suspended', 'failed')),
    failure_count INTEGER NOT NULL DEFAULT 0,
    last_failure_at TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
"#;

const MIGRATION_035: &str = r#"
-- M35: execution_loop_events — structured event log emitted by the agent loop.
-- Phase 1: State Externalization & Observability (additive — zero behavior change).
-- Each row captures one typed event (round_started, guard_fired, convergence_decided,
-- checkpoint_saved, intent_rescored, critic_evaluated, critic_failed) with its full
-- JSON payload for offline analysis and post-mortem debugging.
CREATE TABLE IF NOT EXISTS execution_loop_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT    NOT NULL,
    round       INTEGER NOT NULL,
    event_type  TEXT    NOT NULL,
    event_json  TEXT    NOT NULL,
    emitted_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_loop_events_session
    ON execution_loop_events (session_id, round);
"#;

const MIGRATION_036: &str = r#"
-- M36: daily_user_metrics — per-user per-day usage aggregates for admin analytics API.
-- DECISION: denormalised daily rollup table rather than querying sessions on every admin
-- request. Keeps admin reads O(1) regardless of session table size.
-- Populated by the CLI agent loop at session end (upsert on PRIMARY KEY).
CREATE TABLE IF NOT EXISTS daily_user_metrics (
    date          TEXT NOT NULL,
    user_id       TEXT NOT NULL,
    sessions      INTEGER DEFAULT 0,
    lines_added   INTEGER DEFAULT 0,
    lines_removed INTEGER DEFAULT 0,
    commits       INTEGER DEFAULT 0,
    prs           INTEGER DEFAULT 0,
    tokens_in     INTEGER DEFAULT 0,
    tokens_out    INTEGER DEFAULT 0,
    cost_usd      REAL DEFAULT 0.0,
    PRIMARY KEY (date, user_id)
);
CREATE INDEX IF NOT EXISTS idx_daily_user_metrics_date
    ON daily_user_metrics (date);
CREATE INDEX IF NOT EXISTS idx_daily_user_metrics_user
    ON daily_user_metrics (user_id);
"#;

const MIGRATION_037: &str = r#"
-- M37: mailbox_messages — agent-to-agent P2P messaging within a team.
-- DECISION: stored in SQLite for durability across process restarts and
-- automatic audit trail inclusion. WAL mode (set at DB open) allows
-- multiple concurrent readers (agents) with a single writer.
-- 'broadcast' is the reserved to_agent sentinel for team-wide messages.
CREATE TABLE IF NOT EXISTS mailbox_messages (
    id           TEXT PRIMARY KEY,
    from_agent   TEXT NOT NULL,
    to_agent     TEXT NOT NULL,   -- agent id or 'broadcast'
    team_id      TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    expires_at   TEXT,            -- NULL = never expires
    consumed     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_mailbox_team_to
    ON mailbox_messages(team_id, to_agent, consumed, expires_at);
"#;

/// Run all pending migrations.
pub fn run_migrations(conn: &Connection) -> Result<(), halcon_core::error::HalconError> {
    // Ensure migrations table exists
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );",
    )
    .map_err(|e| halcon_core::error::HalconError::MigrationError(e.to_string()))?;

    let current_version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| halcon_core::error::HalconError::MigrationError(e.to_string()))?;

    for &(version, name, sql) in MIGRATIONS {
        if version > current_version {
            tracing::info!("Running migration {version}: {name}");
            conn.execute_batch(sql).map_err(|e| {
                halcon_core::error::HalconError::MigrationError(format!(
                    "migration {version} ({name}): {e}"
                ))
            })?;

            conn.execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![version, name, chrono::Utc::now().to_rfc3339()],
            )
            .map_err(|e| halcon_core::error::HalconError::MigrationError(e.to_string()))?;
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
        assert_eq!(version, 36);
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
        assert_eq!(count, 36);
    }

    #[test]
    fn migration_033_adds_description_column_to_media_index() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify the description column exists by inserting a row with it.
        conn.execute(
            "INSERT INTO media_index (content_hash, modality, embedding_data, embedding_dim, created_at, description)
             VALUES ('test_hash', 'image', X'00', 1, datetime('now'), 'test description')",
            [],
        ).expect("should insert row with description column");

        let desc: Option<String> = conn
            .query_row(
                "SELECT description FROM media_index WHERE content_hash = 'test_hash'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(desc.as_deref(), Some("test description"));
    }

    #[test]
    fn migration_034_creates_plugin_circuit_state_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify the table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='plugin_circuit_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "plugin_circuit_state table must exist after M34");

        // Verify insert with valid state works.
        conn.execute(
            "INSERT INTO plugin_circuit_state (plugin_id, state, failure_count, last_failure_at)
             VALUES ('test-plugin', 'degraded', 3, '2026-02-21T00:00:00Z')",
            [],
        ).expect("should insert into plugin_circuit_state");

        let (state, count): (String, i32) = conn
            .query_row(
                "SELECT state, failure_count FROM plugin_circuit_state WHERE plugin_id = 'test-plugin'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "degraded");
        assert_eq!(count, 3);

        // Verify state CHECK constraint rejects invalid values.
        let bad_insert = conn.execute(
            "INSERT INTO plugin_circuit_state (plugin_id, state, failure_count) VALUES ('bad', 'unknown_state', 0)",
            [],
        );
        assert!(bad_insert.is_err(), "invalid state must be rejected by CHECK constraint");
    }

    #[test]
    fn migration_035_creates_execution_loop_events_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='execution_loop_events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "execution_loop_events table must exist after M35");

        // Verify index exists.
        let index_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_loop_events_session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(index_exists, "idx_loop_events_session index must exist after M35");

        // Verify insert and retrieval work.
        let session_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO execution_loop_events (session_id, round, event_type, event_json)
             VALUES (?1, 0, 'round_started', '{\"type\":\"round_started\",\"round\":0}')",
            rusqlite::params![session_id],
        ).expect("should insert loop event");

        let (event_type, json): (String, String) = conn
            .query_row(
                "SELECT event_type, event_json FROM execution_loop_events WHERE session_id = ?1",
                rusqlite::params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(event_type, "round_started");
        assert!(json.contains("round_started"), "json={json}");
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

    #[test]
    fn migration_017_creates_reasoning_experience_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify reasoning_experience table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='reasoning_experience'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "reasoning_experience table should exist");

        // Insert an experience record.
        conn.execute(
            "INSERT INTO reasoning_experience (task_type, strategy, avg_score, uses, last_updated)
             VALUES ('CodeGeneration', 'DirectExecution', 0.85, 5, 1234567890)",
            [],
        )
        .unwrap();

        let (avg, uses): (f64, u32) = conn
            .query_row(
                "SELECT avg_score, uses FROM reasoning_experience WHERE task_type = 'CodeGeneration'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((avg - 0.85).abs() < 0.001);
        assert_eq!(uses, 5);
    }

    #[test]
    fn migration_018_creates_permission_rules_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify permission_rules table exists.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='permission_rules'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "permission_rules table should exist");

        // Verify indexes exist.
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_permission_rules_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 4, "should have 4 permission_rules indexes");

        // Insert a permission rule.
        conn.execute(
            "INSERT INTO permission_rules (rule_id, scope, scope_value, tool_pattern, tool_pattern_type, decision, metadata_json, created_at, active)
             VALUES ('rule-1', 'directory', '/tmp', 'bash', 'exact', 'allowed_for_directory', '{}', '2026-02-15T00:00:00Z', 1)",
            [],
        )
        .unwrap();

        let (scope, tool, decision): (String, String, String) = conn
            .query_row(
                "SELECT scope, tool_pattern, decision FROM permission_rules WHERE rule_id = 'rule-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(scope, "directory");
        assert_eq!(tool, "bash");
        assert_eq!(decision, "allowed_for_directory");
    }

    #[test]
    fn migration_026_adds_messages_compressed_column() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify sessions table has messages_compressed column.
        let has_col: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('sessions') WHERE name = 'messages_compressed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_col, "sessions should have messages_compressed column after M26");

        // Verify NULL is accepted (old rows without compression).
        conn.execute(
            "INSERT INTO sessions (id, model, provider, working_directory, messages_json, created_at, updated_at)
             VALUES ('sess-m26', 'echo', 'echo', '/tmp', '[{\"role\":\"user\",\"content\":\"hi\"}]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let compressed: Option<Vec<u8>> = conn
            .query_row(
                "SELECT messages_compressed FROM sessions WHERE id = 'sess-m26'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(compressed.is_none(), "messages_compressed should be NULL for old-style row");
    }

    #[test]
    fn migration_021_creates_activity_search_history_table() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify activity_search_history table exists
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='activity_search_history'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists, "activity_search_history table should exist");

        // Verify indexes exist
        let idx_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_activity_search_history_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 2, "should have 2 activity_search_history indexes");

        // Verify trigger exists
        let trigger_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='trigger' AND name='activity_search_history_cleanup'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(trigger_exists, "cleanup trigger should exist");

        // Insert a search query
        conn.execute(
            "INSERT INTO activity_search_history (query, search_mode, match_count, searched_at, session_id)
             VALUES ('hello world', 'exact', 5, '2026-02-17T10:00:00Z', 'session-123')",
            [],
        )
        .unwrap();

        let (query, mode, count): (String, String, i32) = conn
            .query_row(
                "SELECT query, search_mode, match_count FROM activity_search_history WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(query, "hello world");
        assert_eq!(mode, "exact");
        assert_eq!(count, 5);
    }
}
