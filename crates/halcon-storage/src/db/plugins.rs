//! Plugin persistence — installed plugins and UCB1 metrics storage.
//!
//! Provides CRUD methods on `Database` for:
//! - `installed_plugins` — manifest records for registered V3 plugins
//! - `plugin_metrics` — per-plugin call counts, token usage, and UCB1 reward stats

use chrono::Utc;
use rusqlite::{Connection, Result, params};

use crate::db::Database;

// ─── Structs ─────────────────────────────────────────────────────────────────

/// Represents an installed plugin record in the database.
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub category: String,
    pub manifest_toml: String,
    pub installed_at: String,
    pub trust_level: String,
}

/// Persisted circuit breaker state for one plugin (M34).
///
/// Enables plugins with historical failures to restart in `degraded` state
/// rather than `clean`, preventing repeated invocations of broken plugins
/// across cold restarts.
#[derive(Debug, Clone)]
pub struct CircuitBreakerStateRow {
    pub plugin_id: String,
    /// One of: "clean" | "degraded" | "suspended" | "failed"
    pub state: String,
    pub failure_count: i64,
    pub last_failure_at: Option<String>,
}

/// Per-plugin UCB1 and call metrics for cross-session learning.
#[derive(Debug, Clone)]
pub struct PluginMetricsRecord {
    pub plugin_id: String,
    pub calls_made: i64,
    pub calls_failed: i64,
    pub tokens_used: i64,
    pub ucb1_n_uses: i64,
    pub ucb1_sum_rewards: f64,
    pub updated_at: String,
}

// ─── Database impl ────────────────────────────────────────────────────────────

impl Database {
    /// Insert or update an installed plugin record.
    pub fn save_installed_plugin(&self, plugin: &InstalledPlugin) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| { tracing::error!("db mutex poisoned — recovering"); p.into_inner() });
        save_installed_plugin_conn(&conn, plugin)
    }

    /// Load all installed plugin records.
    pub fn load_installed_plugins(&self) -> Result<Vec<InstalledPlugin>> {
        let conn = self.conn.lock().unwrap_or_else(|p| { tracing::error!("db mutex poisoned — recovering"); p.into_inner() });
        load_installed_plugins_conn(&conn)
    }

    /// Delete an installed plugin record by plugin_id.
    pub fn delete_installed_plugin(&self, plugin_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| { tracing::error!("db mutex poisoned — recovering"); p.into_inner() });
        conn.execute(
            "DELETE FROM installed_plugins WHERE plugin_id = ?1",
            params![plugin_id],
        )?;
        Ok(())
    }

    /// Upsert plugin metrics (UCB1 + call counts).
    pub fn save_plugin_metrics(&self, metrics: &[PluginMetricsRecord]) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| { tracing::error!("db mutex poisoned — recovering"); p.into_inner() });
        save_plugin_metrics_conn(&conn, metrics)
    }

    /// Load all plugin metrics records.
    pub fn load_plugin_metrics(&self) -> Result<Vec<PluginMetricsRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|p| { tracing::error!("db mutex poisoned — recovering"); p.into_inner() });
        load_plugin_metrics_conn(&conn)
    }

    /// Upsert circuit breaker state for a batch of plugins (M34).
    ///
    /// Called post-loop to persist the current in-memory circuit state so that
    /// the next session can restore it via `load_circuit_breaker_states()`.
    pub fn save_circuit_breaker_states(
        &self,
        rows: &[CircuitBreakerStateRow],
    ) -> halcon_core::error::Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| { tracing::error!("db mutex poisoned — recovering"); p.into_inner() });
        let now = Utc::now().to_rfc3339();
        for row in rows {
            conn.execute(
                "INSERT INTO plugin_circuit_state
                 (plugin_id, state, failure_count, last_failure_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(plugin_id) DO UPDATE SET
                     state           = ?2,
                     failure_count   = ?3,
                     last_failure_at = ?4,
                     updated_at      = ?5",
                params![
                    row.plugin_id,
                    row.state,
                    row.failure_count,
                    row.last_failure_at,
                    now,
                ],
            )
            .map_err(|e| halcon_core::error::HalconError::DatabaseError(format!("save_circuit_breaker_states: {e}")))?;
        }
        Ok(())
    }

    /// Load all persisted circuit breaker states (M34).
    ///
    /// Called at startup to seed plugin states; plugins with historical failures
    /// are initialized in `degraded` state rather than `clean`.
    pub fn load_circuit_breaker_states(
        &self,
    ) -> halcon_core::error::Result<Vec<CircuitBreakerStateRow>> {
        let conn = self.conn.lock().unwrap_or_else(|p| { tracing::error!("db mutex poisoned — recovering"); p.into_inner() });
        let mut stmt = conn
            .prepare(
                "SELECT plugin_id, state, failure_count, last_failure_at
                 FROM plugin_circuit_state
                 ORDER BY plugin_id ASC",
            )
            .map_err(|e| halcon_core::error::HalconError::DatabaseError(format!("prepare load_circuit_breaker_states: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(CircuitBreakerStateRow {
                    plugin_id:       row.get(0)?,
                    state:           row.get(1)?,
                    failure_count:   row.get(2)?,
                    last_failure_at: row.get(3)?,
                })
            })
            .map_err(|e| halcon_core::error::HalconError::DatabaseError(format!("query load_circuit_breaker_states: {e}")))?
            .filter_map(|r| match r {
                Ok(row) => Some(row),
                Err(ref e) => {
                    // R-03: Log and discard malformed rows rather than silently dropping them.
                    // A corrupt or partially-migrated row must never silently reset a breaker
                    // from Open → Closed, which would allow a flood of requests to a failing provider.
                    tracing::error!(
                        target: "halcon::db::plugins",
                        error = %e,
                        "circuit_breaker_state_parse_failed: row discarded. \
                         Provider may be initialised in Closed state instead of its persisted state."
                    );
                    None
                }
            })
            .collect();
        Ok(rows)
    }
}

// ─── Free functions (connection-level) ───────────────────────────────────────

pub(crate) fn save_installed_plugin_conn(
    conn: &Connection,
    plugin: &InstalledPlugin,
) -> Result<()> {
    conn.execute(
        "INSERT INTO installed_plugins
         (plugin_id, name, version, category, manifest_toml, installed_at, trust_level)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(plugin_id) DO UPDATE SET
             name          = ?2,
             version       = ?3,
             category      = ?4,
             manifest_toml = ?5,
             trust_level   = ?7",
        params![
            plugin.plugin_id,
            plugin.name,
            plugin.version,
            plugin.category,
            plugin.manifest_toml,
            plugin.installed_at,
            plugin.trust_level,
        ],
    )?;
    Ok(())
}

pub(crate) fn load_installed_plugins_conn(conn: &Connection) -> Result<Vec<InstalledPlugin>> {
    let mut stmt = conn.prepare(
        "SELECT plugin_id, name, version, category, manifest_toml, installed_at, trust_level
         FROM installed_plugins
         ORDER BY installed_at ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(InstalledPlugin {
            plugin_id:     row.get(0)?,
            name:          row.get(1)?,
            version:       row.get(2)?,
            category:      row.get(3)?,
            manifest_toml: row.get(4)?,
            installed_at:  row.get(5)?,
            trust_level:   row.get(6)?,
        })
    })?;
    rows.collect()
}

pub(crate) fn save_plugin_metrics_conn(
    conn: &Connection,
    metrics: &[PluginMetricsRecord],
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    for m in metrics {
        conn.execute(
            "INSERT INTO plugin_metrics
             (plugin_id, calls_made, calls_failed, tokens_used, ucb1_n_uses, ucb1_sum_rewards, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(plugin_id) DO UPDATE SET
                 calls_made       = ?2,
                 calls_failed     = ?3,
                 tokens_used      = ?4,
                 ucb1_n_uses      = ?5,
                 ucb1_sum_rewards = ?6,
                 updated_at       = ?7",
            params![
                m.plugin_id,
                m.calls_made,
                m.calls_failed,
                m.tokens_used,
                m.ucb1_n_uses,
                m.ucb1_sum_rewards,
                now,
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn load_plugin_metrics_conn(conn: &Connection) -> Result<Vec<PluginMetricsRecord>> {
    let mut stmt = conn.prepare(
        "SELECT plugin_id, calls_made, calls_failed, tokens_used,
                ucb1_n_uses, ucb1_sum_rewards, updated_at
         FROM plugin_metrics
         ORDER BY plugin_id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PluginMetricsRecord {
            plugin_id:        row.get(0)?,
            calls_made:       row.get(1)?,
            calls_failed:     row.get(2)?,
            tokens_used:      row.get(3)?,
            ucb1_n_uses:      row.get(4)?,
            ucb1_sum_rewards: row.get(5)?,
            updated_at:       row.get(6)?,
        })
    })?;
    rows.collect()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn make_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    fn make_plugin(id: &str) -> InstalledPlugin {
        InstalledPlugin {
            plugin_id:     id.to_string(),
            name:          format!("Plugin {id}"),
            version:       "1.0.0".to_string(),
            category:      "development".to_string(),
            manifest_toml: format!("[meta]\nid = \"{id}\""),
            installed_at:  Utc::now().to_rfc3339(),
            trust_level:   "local".to_string(),
        }
    }

    #[test]
    fn save_and_load_installed_plugin() {
        let db = make_db();
        let plugin = make_plugin("my-plugin");
        db.save_installed_plugin(&plugin).unwrap();

        let loaded = db.load_installed_plugins().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].plugin_id, "my-plugin");
        assert_eq!(loaded[0].name, "Plugin my-plugin");
        assert_eq!(loaded[0].trust_level, "local");
    }

    #[test]
    fn save_plugin_is_upsert() {
        let db = make_db();
        let plugin = make_plugin("upsert-plugin");
        db.save_installed_plugin(&plugin).unwrap();

        // Update version
        let mut updated = plugin.clone();
        updated.version = "2.0.0".to_string();
        db.save_installed_plugin(&updated).unwrap();

        let loaded = db.load_installed_plugins().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].version, "2.0.0");
    }

    #[test]
    fn delete_installed_plugin() {
        let db = make_db();
        db.save_installed_plugin(&make_plugin("delete-me")).unwrap();
        db.save_installed_plugin(&make_plugin("keep-me")).unwrap();

        db.delete_installed_plugin("delete-me").unwrap();

        let loaded = db.load_installed_plugins().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].plugin_id, "keep-me");
    }

    #[test]
    fn save_and_load_plugin_metrics() {
        let db = make_db();
        let metrics = vec![PluginMetricsRecord {
            plugin_id:        "metric-plugin".to_string(),
            calls_made:       10,
            calls_failed:     2,
            tokens_used:      5000,
            ucb1_n_uses:      10,
            ucb1_sum_rewards: 7.5,
            updated_at:       Utc::now().to_rfc3339(),
        }];
        db.save_plugin_metrics(&metrics).unwrap();

        let loaded = db.load_plugin_metrics().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].plugin_id, "metric-plugin");
        assert_eq!(loaded[0].calls_made, 10);
        assert_eq!(loaded[0].calls_failed, 2);
        assert_eq!(loaded[0].tokens_used, 5000);
        assert_eq!(loaded[0].ucb1_n_uses, 10);
        assert!((loaded[0].ucb1_sum_rewards - 7.5).abs() < 1e-9);
    }

    #[test]
    fn plugin_metrics_upsert_updates_counts() {
        let db = make_db();
        let initial = vec![PluginMetricsRecord {
            plugin_id:        "evolving-plugin".to_string(),
            calls_made:       5,
            calls_failed:     0,
            tokens_used:      1000,
            ucb1_n_uses:      5,
            ucb1_sum_rewards: 4.0,
            updated_at:       Utc::now().to_rfc3339(),
        }];
        db.save_plugin_metrics(&initial).unwrap();

        let updated = vec![PluginMetricsRecord {
            plugin_id:        "evolving-plugin".to_string(),
            calls_made:       10,
            calls_failed:     1,
            tokens_used:      2500,
            ucb1_n_uses:      10,
            ucb1_sum_rewards: 8.5,
            updated_at:       Utc::now().to_rfc3339(),
        }];
        db.save_plugin_metrics(&updated).unwrap();

        let loaded = db.load_plugin_metrics().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].calls_made, 10);
        assert_eq!(loaded[0].ucb1_n_uses, 10);
        assert!((loaded[0].ucb1_sum_rewards - 8.5).abs() < 1e-9);
    }

    #[test]
    fn load_empty_returns_empty_vecs() {
        let db = make_db();
        assert!(db.load_installed_plugins().unwrap().is_empty());
        assert!(db.load_plugin_metrics().unwrap().is_empty());
        assert!(db.load_circuit_breaker_states().unwrap().is_empty());
    }

    #[test]
    fn save_and_load_circuit_breaker_state() {
        let db = make_db();
        let rows = vec![
            CircuitBreakerStateRow {
                plugin_id: "alpha".to_string(),
                state: "degraded".to_string(),
                failure_count: 2,
                last_failure_at: Some("2026-02-21T00:00:00Z".to_string()),
            },
            CircuitBreakerStateRow {
                plugin_id: "beta".to_string(),
                state: "clean".to_string(),
                failure_count: 0,
                last_failure_at: None,
            },
        ];
        db.save_circuit_breaker_states(&rows).unwrap();

        let loaded = db.load_circuit_breaker_states().unwrap();
        assert_eq!(loaded.len(), 2);

        let alpha = loaded.iter().find(|r| r.plugin_id == "alpha").unwrap();
        assert_eq!(alpha.state, "degraded");
        assert_eq!(alpha.failure_count, 2);
        assert_eq!(alpha.last_failure_at.as_deref(), Some("2026-02-21T00:00:00Z"));

        let beta = loaded.iter().find(|r| r.plugin_id == "beta").unwrap();
        assert_eq!(beta.state, "clean");
        assert_eq!(beta.failure_count, 0);
        assert!(beta.last_failure_at.is_none());
    }

    #[test]
    fn circuit_state_upsert_updates_on_conflict() {
        let db = make_db();
        let initial = vec![CircuitBreakerStateRow {
            plugin_id: "my-plugin".to_string(),
            state: "clean".to_string(),
            failure_count: 0,
            last_failure_at: None,
        }];
        db.save_circuit_breaker_states(&initial).unwrap();

        // Now update to degraded after a failure
        let updated = vec![CircuitBreakerStateRow {
            plugin_id: "my-plugin".to_string(),
            state: "degraded".to_string(),
            failure_count: 1,
            last_failure_at: Some("2026-02-21T01:00:00Z".to_string()),
        }];
        db.save_circuit_breaker_states(&updated).unwrap();

        let loaded = db.load_circuit_breaker_states().unwrap();
        assert_eq!(loaded.len(), 1, "upsert must not duplicate");
        assert_eq!(loaded[0].state, "degraded");
        assert_eq!(loaded[0].failure_count, 1);
    }
}
