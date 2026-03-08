//! Global SearchEngine singleton for native search functionality.

use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use halcon_search::{SearchEngine, SearchEngineConfig};
use halcon_storage::Database;

/// Global SearchEngine singleton.
/// Uses tokio::sync::Mutex so execute_search() can hold the lock across .await (COUPLING-001 fix).
static SEARCH_ENGINE: OnceLock<Arc<Mutex<Option<SearchEngine>>>> = OnceLock::new();

/// Initialize the global search engine with database and configuration.
///
/// Must be called before any search operations. Can be called multiple times -
/// subsequent calls are no-ops.
pub fn init_search_engine(db: Arc<Database>, config: SearchEngineConfig) {
    SEARCH_ENGINE.get_or_init(|| {
        match SearchEngine::new(db, config) {
            Ok(engine) => {
                tracing::info!("Native search engine initialized");
                Arc::new(Mutex::new(Some(engine)))
            }
            Err(e) => {
                tracing::error!("Failed to initialize search engine: {e}");
                Arc::new(Mutex::new(None))
            }
        }
    });
}

/// Get a reference to the global search engine.
///
/// Returns None if the engine has not been initialized or initialization failed.
pub fn get_search_engine() -> Option<Arc<Mutex<Option<SearchEngine>>>> {
    SEARCH_ENGINE.get().cloned()
}

/// Execute a search query using the global search engine.
///
/// Returns an error if the engine is not initialized.
pub async fn execute_search(query: &str) -> halcon_search::Result<halcon_search::types::SearchResults> {
    let engine_lock = get_search_engine()
        .ok_or_else(|| halcon_search::SearchError::ConfigError("Search engine not initialized".to_string()))?;

    // COUPLING-001 fix: tokio::sync::Mutex allows holding the guard across .await safely.
    let engine_guard = engine_lock.lock().await;
    let engine = engine_guard.as_ref()
        .ok_or_else(|| halcon_search::SearchError::ConfigError("Search engine initialization failed".to_string()))?;

    engine.search(query).await
}
