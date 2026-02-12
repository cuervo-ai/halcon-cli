//! CLI commands for managing the persistent memory store.

use anyhow::Result;
use cuervo_storage::Database;

use crate::config_loader::default_db_path;
use cuervo_core::types::AppConfig;
use cuervo_storage::MemoryEntryType;

/// Open the database (best-effort).
fn open_db(config: &AppConfig) -> Result<Database> {
    let db_path = config
        .storage
        .database_path
        .clone()
        .unwrap_or_else(default_db_path);
    Database::open(&db_path).map_err(|e| anyhow::anyhow!("Could not open database: {e}"))
}

/// List memory entries, optionally filtered by type.
pub fn list(config: &AppConfig, entry_type: Option<&str>, limit: u32) -> Result<()> {
    let db = open_db(config)?;

    let et = entry_type
        .map(|s| {
            MemoryEntryType::parse(s)
                .ok_or_else(|| anyhow::anyhow!("Unknown memory type: {s}. Valid types: fact, session_summary, decision, code_snippet, project_meta"))
        })
        .transpose()?;

    let entries = db
        .list_memories(et, limit)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if entries.is_empty() {
        println!("No memory entries found.");
        return Ok(());
    }

    println!("{:<36}  {:<16}  {:<20}  Content", "ID", "Type", "Created");
    println!("{}", "-".repeat(100));

    for entry in &entries {
        let id = &entry.entry_id.to_string()[..8];
        let entry_type = entry.entry_type.as_str();
        let created = entry.created_at.format("%Y-%m-%d %H:%M");
        let content: String = entry.content.chars().take(50).collect();
        let content = content.replace('\n', " ");
        println!("{:<36}  {:<16}  {:<20}  {}", id, entry_type, created, content);
    }

    println!("\n{} entries shown.", entries.len());
    Ok(())
}

/// Search memory entries by keyword (BM25 full-text search).
pub fn search(config: &AppConfig, query: &str, limit: usize) -> Result<()> {
    let db = open_db(config)?;

    let entries = db
        .search_memory_fts(query, limit)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if entries.is_empty() {
        println!("No results for \"{query}\".");
        return Ok(());
    }

    println!("Results for \"{}\":\n", query);

    for (i, entry) in entries.iter().enumerate() {
        let id = &entry.entry_id.to_string()[..8];
        let entry_type = entry.entry_type.as_str();
        println!("{}. [{}] {} ({})", i + 1, entry_type, entry.content, id);
    }

    println!("\n{} results.", entries.len());
    Ok(())
}

/// Prune expired and excess memory entries.
///
/// When `force` is false, shows a preview of what would be pruned and asks for
/// confirmation before deleting. When `force` is true, prunes immediately.
pub fn prune(config: &AppConfig, force: bool) -> Result<()> {
    let db = open_db(config)?;

    // Show current stats before pruning.
    let stats = db.memory_stats().map_err(|e| anyhow::anyhow!("{e}"))?;
    let max = config.memory.max_entries;

    if !force {
        println!(
            "Memory: {} entries (max: {})",
            stats.total_entries, max
        );
        if stats.total_entries <= max {
            println!("Nothing to prune — within limits.");
            return Ok(());
        }
        let would_remove = stats.total_entries - max;
        eprint!("This will remove ~{would_remove} entries. Continue? [y/N]: ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim().to_lowercase() != "y" {
            println!("Aborted.");
            return Ok(());
        }
    }

    let removed = db
        .prune_memories(max, Some(chrono::Utc::now()))
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if removed > 0 {
        println!("Pruned {removed} memory entries.");
    } else {
        println!("Nothing to prune.");
    }

    Ok(())
}

/// Show memory store statistics.
pub fn stats(config: &AppConfig) -> Result<()> {
    let db = open_db(config)?;

    let stats = db.memory_stats().map_err(|e| anyhow::anyhow!("{e}"))?;

    println!("Memory Statistics:");
    println!("  Total entries: {}", stats.total_entries);

    if !stats.by_type.is_empty() {
        println!("  By type:");
        for (t, count) in &stats.by_type {
            println!("    {:<20} {}", t, count);
        }
    }

    if let Some(oldest) = stats.oldest_entry {
        println!("  Oldest: {}", oldest.format("%Y-%m-%d %H:%M:%S"));
    }
    if let Some(newest) = stats.newest_entry {
        println!("  Newest: {}", newest.format("%Y-%m-%d %H:%M:%S"));
    }

    println!(
        "  Config: max_entries={}, ttl={}, auto_summarize={}",
        config.memory.max_entries,
        config
            .memory
            .default_ttl_days
            .map(|d| format!("{d}d"))
            .unwrap_or_else(|| "none".to_string()),
        config.memory.auto_summarize,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_entry_type_parse_valid() {
        assert!(MemoryEntryType::parse("fact").is_some());
        assert!(MemoryEntryType::parse("session_summary").is_some());
        assert!(MemoryEntryType::parse("decision").is_some());
        assert!(MemoryEntryType::parse("code_snippet").is_some());
        assert!(MemoryEntryType::parse("project_meta").is_some());
    }

    #[test]
    fn memory_entry_type_parse_invalid() {
        assert!(MemoryEntryType::parse("unknown_type").is_none());
        assert!(MemoryEntryType::parse("").is_none());
        assert!(MemoryEntryType::parse("FACT").is_none()); // case-sensitive
    }

    #[test]
    fn list_with_in_memory_db_empty() {
        let mut config = AppConfig::default();
        config.storage.database_path = Some(":memory:".into());
        // list should succeed with empty memory.
        let result = list(&config, None, 10);
        assert!(result.is_ok());
    }

    #[test]
    fn search_with_in_memory_db_empty() {
        let mut config = AppConfig::default();
        config.storage.database_path = Some(":memory:".into());
        let result = search(&config, "test", 10);
        assert!(result.is_ok());
    }
}
