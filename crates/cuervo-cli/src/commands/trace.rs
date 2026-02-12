use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use uuid::Uuid;

use cuervo_storage::{AsyncDatabase, Database, TraceStepType};
use cuervo_tools::ToolRegistry;

use crate::config_loader::default_db_path;

/// Export a session's trace as deterministic JSON to stdout.
pub fn export(session_id: &str, db_path: Option<&Path>) -> Result<()> {
    let id = Uuid::parse_str(session_id)
        .map_err(|e| anyhow::anyhow!("Invalid session ID: {e}"))?;

    let path = db_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_db_path);

    let db = Database::open(&path)
        .context("Failed to open database")?;

    let export = db
        .export_trace(id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if export.steps.is_empty() {
        eprintln!("No trace steps found for session {session_id}.");
        return Ok(());
    }

    let json = serde_json::to_string_pretty(&export)
        .context("Failed to serialize trace")?;
    println!("{json}");
    Ok(())
}

/// Replay a session's trace, rendering steps as if they happened live.
///
/// When `verify` is true, runs a deterministic replay via `run_replay()`
/// and compares execution fingerprints. When false, renders the trace steps
/// as text output (backward-compatible visualization mode).
pub async fn replay(session_id: &str, db_path: Option<&Path>, verify: bool) -> Result<()> {
    let id = Uuid::parse_str(session_id)
        .map_err(|e| anyhow::anyhow!("Invalid session ID: {e}"))?;

    let path = db_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_db_path);

    let db = Database::open(&path)
        .context("Failed to open database")?;

    if verify {
        return replay_verify(id, &db).await;
    }

    // Visualization mode (default).
    replay_visualize(id, session_id, &db)
}

/// Deterministic replay with fingerprint verification.
async fn replay_verify(session_id: Uuid, db: &Database) -> Result<()> {
    let async_db = AsyncDatabase::new(Arc::new(
        Database::open(db.path()).context("Failed to open database for replay")?,
    ));
    let tool_registry = ToolRegistry::new();
    let (event_tx, _rx) = cuervo_core::event_bus(16);

    let short_id = &session_id.to_string()[..8];
    eprintln!("Replaying session {short_id} with verification...\n");

    let result = crate::repl::replay_runner::run_replay(
        session_id,
        &async_db,
        &tool_registry,
        &event_tx,
        true,
    )
    .await?;

    eprintln!("Replay complete:");
    eprintln!("  Original session:  {}", &result.original_session_id.to_string()[..8]);
    eprintln!("  Replay session:    {}", &result.replay_session_id.to_string()[..8]);
    eprintln!("  Steps replayed:    {}", result.steps_replayed);
    eprintln!("  Rounds:            {}", result.rounds);
    eprintln!("  Replay fingerprint: {}", &result.replay_fingerprint[..16]);

    if let Some(ref orig_fp) = result.original_fingerprint {
        eprintln!("  Original fingerprint: {}", &orig_fp[..orig_fp.len().min(16)]);
    } else {
        eprintln!("  Original fingerprint: (not recorded)");
    }

    if result.fingerprint_match {
        eprintln!("\n  Fingerprint: MATCH");
    } else if result.original_fingerprint.is_some() {
        eprintln!("\n  Fingerprint: MISMATCH");
        std::process::exit(1);
    } else {
        eprintln!("\n  Fingerprint: UNVERIFIED (original not recorded)");
    }

    Ok(())
}

/// Visualization-only replay (renders trace steps as text).
fn replay_visualize(id: Uuid, session_id: &str, db: &Database) -> Result<()> {
    let steps = db
        .load_trace_steps(id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if steps.is_empty() {
        eprintln!("No trace steps found for session {session_id}.");
        return Ok(());
    }

    let short_id = &session_id[..session_id.len().min(8)];
    eprintln!("Replaying session {short_id} ({} steps)...\n", steps.len());

    for step in &steps {
        match step.step_type {
            TraceStepType::ModelRequest => {
                let data: serde_json::Value =
                    serde_json::from_str(&step.data_json).unwrap_or_default();
                let round = data.get("round").and_then(|v| v.as_u64()).unwrap_or(0);
                let model = data
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let msg_count = data
                    .get("message_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let tool_count = data
                    .get("tool_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                eprintln!(
                    "[step {}] ModelRequest  round={round} model={model} messages={msg_count} tools={tool_count}",
                    step.step_index
                );
            }
            TraceStepType::ModelResponse => {
                let data: serde_json::Value =
                    serde_json::from_str(&step.data_json).unwrap_or_default();
                let text = data
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let stop = data
                    .get("stop_reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let latency = step.duration_ms;

                eprintln!(
                    "[step {}] ModelResponse stop={stop} latency={latency}ms",
                    step.step_index
                );
                if !text.is_empty() {
                    // Render the model text to stdout (as if live).
                    println!("{text}");
                }
            }
            TraceStepType::ToolCall => {
                let data: serde_json::Value =
                    serde_json::from_str(&step.data_json).unwrap_or_default();
                let name = data
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let input = data.get("input").cloned().unwrap_or_default();
                eprintln!(
                    "[step {}] ToolCall      tool={name} input={input}",
                    step.step_index
                );
            }
            TraceStepType::ToolResult => {
                let data: serde_json::Value =
                    serde_json::from_str(&step.data_json).unwrap_or_default();
                let name = data
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let is_error = data
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let duration = step.duration_ms;
                let status = if is_error { "ERROR" } else { "OK" };
                eprintln!(
                    "[step {}] ToolResult    tool={name} status={status} duration={duration}ms",
                    step.step_index
                );
            }
            TraceStepType::Error => {
                let data: serde_json::Value =
                    serde_json::from_str(&step.data_json).unwrap_or_default();
                let msg = data
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                let ctx = data
                    .get("context")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                eprintln!(
                    "[step {}] Error         context={ctx} message={msg}",
                    step.step_index
                );
            }
        }
    }

    eprintln!("\nReplay complete ({} steps).", steps.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_invalid_session_id_errors() {
        let result = export("not-a-uuid", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid session ID"));
    }

    #[test]
    fn export_missing_session_errors() {
        // Use a temp in-memory db via a temp file.
        let dir = std::env::temp_dir().join("cuervo_test_export");
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("test_export.db");
        let _ = Database::open(&db_path); // create DB
        let result = export(&Uuid::new_v4().to_string(), Some(&db_path));
        // Should succeed but print "no trace steps".
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn replay_invalid_session_id_errors() {
        let result = replay("not-a-uuid", None, false).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid session ID"));
    }

    #[test]
    fn uuid_parsing_valid() {
        let id = Uuid::new_v4();
        let parsed = Uuid::parse_str(&id.to_string());
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap(), id);
    }
}
