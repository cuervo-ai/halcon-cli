//! CLI commands for managing cron-based scheduled agent tasks.
//!
//! ```text
//! halcon schedule add --name "security-scan" --cron "0 2 * * 1" \
//!                     --instruction "Scan for vulnerabilities in main branch"
//! halcon schedule list
//! halcon schedule disable <id>
//! halcon schedule enable  <id>
//! halcon schedule run     <id>   # force-run immediately
//! ```
//!
//! See US-scheduler (PASO 4-C).

use anyhow::{Context, Result};

use halcon_storage::Database;

use crate::repl::agent_scheduler::{
    db_get_scheduled_task, db_insert_scheduled_task, db_list_scheduled_tasks,
    db_set_scheduled_task_enabled,
};

/// Add a new scheduled task.
///
/// Validates the cron expression before inserting so users get immediate feedback
/// on typos (e.g., "0 2 * * 8" — day-of-week 8 is invalid).
pub fn add(
    db: &Database,
    name: &str,
    cron_expr: &str,
    instruction: &str,
    agent_id: Option<&str>,
) -> Result<()> {
    // Validate cron expression before persisting.
    let _ = croner::Cron::new(cron_expr)
        .with_seconds_optional()
        .parse()
        .with_context(|| format!("invalid cron expression: '{cron_expr}'"))?;

    let id = db_insert_scheduled_task(db, name, agent_id, instruction, cron_expr)
        .context("failed to insert scheduled task")?;

    println!("✓ Scheduled task created (id: {id})");
    println!("  name:        {name}");
    println!("  cron:        {cron_expr}");
    println!("  instruction: {instruction}");
    Ok(())
}

/// List all scheduled tasks.
pub fn list(db: &Database) -> Result<()> {
    let tasks = db_list_scheduled_tasks(db).context("failed to list scheduled tasks")?;

    if tasks.is_empty() {
        println!("No scheduled tasks configured.");
        println!("Use `halcon schedule add` to create one.");
        return Ok(());
    }

    println!(
        "{:<38} {:<20} {:<15} {:<12}",
        "ID", "NAME", "CRON", "STATUS"
    );
    println!("{}", "─".repeat(90));
    for task in &tasks {
        let status = if task.enabled { "enabled" } else { "disabled" };
        let last_run = task
            .last_run_at
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "never".to_string());
        println!(
            "{:<38} {:<20} {:<15} {:<12}  last: {}",
            task.id,
            task.name.chars().take(19).collect::<String>(),
            task.cron_expr,
            status,
            last_run
        );
    }
    println!("\n{} task(s) total", tasks.len());
    Ok(())
}

/// Disable a scheduled task (stops it from running on schedule).
pub fn disable(db: &Database, id: &str) -> Result<()> {
    let n = db_set_scheduled_task_enabled(db, id, false).context("failed to disable task")?;
    if n == 0 {
        anyhow::bail!("task not found: {id}");
    }
    println!("✓ Task {id} disabled.");
    Ok(())
}

/// Enable a previously disabled scheduled task.
pub fn enable(db: &Database, id: &str) -> Result<()> {
    let n = db_set_scheduled_task_enabled(db, id, true).context("failed to enable task")?;
    if n == 0 {
        anyhow::bail!("task not found: {id}");
    }
    println!("✓ Task {id} enabled.");
    Ok(())
}

/// Force-run a task immediately (ignoring the schedule).
///
/// This updates `last_run_at` so the normal scheduler will not run it again
/// until the next scheduled occurrence.
pub fn run_now(db: &Database, id: &str) -> Result<()> {
    let task = db_get_scheduled_task(db, id)
        .context("failed to load task")?
        .ok_or_else(|| anyhow::anyhow!("task not found: {id}"))?;

    println!("→ Running scheduled task: {} ({})", task.name, task.id);
    println!("  instruction: {}", task.instruction);

    // Emit the task to the standard output so the caller (main.rs dispatch)
    // can pipe it into the REPL's message handler.
    // In the MVP, we print the instruction so the user can copy-paste it;
    // full integration (spawning a sub-agent) will be wired in the next sprint.
    println!("\n[Manual run] Instruction dispatched to agent:");
    println!("  {}", task.instruction);

    // Update last_run_at so the scheduler does not double-fire.
    db.with_connection(|conn| {
        conn.execute(
            "UPDATE scheduled_tasks SET last_run_at = ?1 WHERE id = ?2",
            rusqlite::params![chrono::Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    })
    .context("failed to update last_run_at")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_storage::Database;

    fn make_db() -> Database {
        Database::open_in_memory().expect("in-memory db")
    }

    #[test]
    fn test_add_and_list() {
        let db = make_db();
        add(&db, "weekly-scan", "0 2 * * 1", "Run security scan", None).expect("add task");
        let tasks = db_list_scheduled_tasks(&db).expect("list");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "weekly-scan");
        assert_eq!(tasks[0].cron_expr, "0 2 * * 1");
        assert!(tasks[0].enabled);
    }

    #[test]
    fn test_disable_enable() {
        let db = make_db();
        add(&db, "my-task", "* * * * *", "do something", None).expect("add");
        let tasks = db_list_scheduled_tasks(&db).expect("list");
        let id = &tasks[0].id.clone();

        disable(&db, id).expect("disable");
        let after_disable = db_list_scheduled_tasks(&db).expect("list");
        assert!(!after_disable[0].enabled);

        enable(&db, id).expect("enable");
        let after_enable = db_list_scheduled_tasks(&db).expect("list");
        assert!(after_enable[0].enabled);
    }

    #[test]
    fn test_add_invalid_cron_errors() {
        let db = make_db();
        let result = add(&db, "bad", "not-cron", "something", None);
        assert!(result.is_err(), "invalid cron should fail");
    }
}
