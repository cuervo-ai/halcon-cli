//! Cached preflight validation of tool JSON schemas.
//!
//! Validates each tool's `input_schema` exactly once per process lifetime and
//! caches the result by tool name.  Subsequent round invocations are O(1) cache
//! lookups — no repeated JSON traversal.
//!
//! Invalid schemas are logged as `warn!` and **excluded** from the returned tool
//! list so the agent loop never sends a broken schema to the provider API, which
//! would surface as a confusing `invalid function definition` error.
//!
//! # Validation rules
//! A schema is valid when it:
//! 1. Is a JSON object (`{}`).
//! 2. Contains a `"type"` key.
//! 3. Has every field named in `"required"` present in `"properties"` (if both
//!    keys exist).

use std::collections::HashSet;
use std::sync::Mutex;

use halcon_core::types::ToolDefinition;

/// Per-process cache mapping tool name → `true` (valid-schema).
///
/// Only *valid* names are stored.  An absent entry means the tool was either
/// never seen or its schema was rejected.  On first access the set is empty;
/// names are inserted after a successful `is_schema_valid()` call.
static VALID_TOOL_CACHE: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

/// Return `true` when `tool.input_schema` satisfies the minimum structural
/// requirements expected by all supported provider APIs.
fn is_schema_valid(tool: &ToolDefinition) -> bool {
    let schema = &tool.input_schema;

    // Rule 1: must be a JSON object.
    let Some(obj) = schema.as_object() else {
        tracing::warn!(
            tool = %tool.name,
            "preflight_validate: input_schema is not a JSON object — tool excluded"
        );
        return false;
    };

    // Rule 2: must contain a "type" key.
    if !obj.contains_key("type") {
        tracing::warn!(
            tool = %tool.name,
            "preflight_validate: input_schema missing required 'type' field — tool excluded"
        );
        return false;
    }

    // Rule 3: every field in "required" must exist in "properties".
    if let (Some(required), Some(properties)) = (
        obj.get("required").and_then(|r| r.as_array()),
        obj.get("properties").and_then(|p| p.as_object()),
    ) {
        for item in required {
            if let Some(field_name) = item.as_str() {
                if !properties.contains_key(field_name) {
                    tracing::warn!(
                        tool = %tool.name,
                        field = field_name,
                        "preflight_validate: required field not in properties — tool excluded"
                    );
                    return false;
                }
            }
        }
    }

    true
}

/// Filter `tools` to only those with structurally valid schemas.
///
/// Each tool name is validated **at most once** across the process lifetime.
/// After the first call the cache is warm and subsequent calls for the same
/// tool names are O(1) `HashSet::contains` lookups.
///
/// # Failures
/// If the internal cache `Mutex` is poisoned (should never happen in practice),
/// the function short-circuits and returns all tools unfiltered, preferring
/// availability over correctness.
pub(crate) fn preflight_validate(tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    let mut cache = match VALID_TOOL_CACHE.lock() {
        Ok(c) => c,
        Err(_) => {
            tracing::warn!("schema_validator: cache lock poisoned — skipping validation");
            return tools;
        }
    };

    let mut valid = Vec::with_capacity(tools.len());
    for tool in tools {
        if cache.contains(&tool.name) {
            // Already known-good — O(1) hit.
            valid.push(tool);
        } else if is_schema_valid(&tool) {
            // First time seen and valid — cache and keep.
            cache.insert(tool.name.clone());
            valid.push(tool);
        }
        // else: invalid schema — excluded; warning already emitted inside is_schema_valid().
    }
    valid
}

/// Clear the validation cache.  Only used in tests to reset inter-test state.
#[cfg(test)]
pub(crate) fn clear_cache() {
    if let Ok(mut c) = VALID_TOOL_CACHE.lock() {
        c.clear();
    }
}

/// Return the number of cached valid-tool names.  Used in tests and diagnostics.
#[cfg(test)]
pub(crate) fn cache_size() -> usize {
    VALID_TOOL_CACHE.lock().map(|c| c.len()).unwrap_or(0)
}

/// Return `true` when the named tool is in the valid-tool cache.
/// Checks by name rather than total size, making it parallel-test-safe.
#[cfg(test)]
pub(crate) fn cache_contains(name: &str) -> bool {
    VALID_TOOL_CACHE.lock().map(|c| c.contains(name)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn make_tool(name: &str, schema: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("Tool {name}"),
            input_schema: schema,
        }
    }

    fn valid_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path" }
            },
            "required": ["path"]
        })
    }

    // Helper: run preflight_validate in isolation by clearing the cache before the
    // call.  The post-clear has been intentionally removed: flushing the cache
    // *after* returning can race with concurrent tests that check `cache_contains()`
    // on their own uniquely-named tools (e.g. `cache_concurrent_no_cross_contamination`),
    // causing non-deterministic failures under `--test-threads=16`.
    // A pre-clear is sufficient for isolation because each test's return value is
    // determined inside `preflight_validate` and cannot be affected by a later clear.
    fn isolated_validate(tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        clear_cache();
        preflight_validate(tools)
    }

    // ── Rule 1: must be a JSON object ────────────────────────────────────────

    #[test]
    fn rejects_non_object_schema_string() {
        let tools = vec![make_tool("bad_string", json!("not an object"))];
        let result = isolated_validate(tools);
        assert!(result.is_empty(), "string schema should be rejected");
    }

    #[test]
    fn rejects_non_object_schema_array() {
        let tools = vec![make_tool("bad_array", json!([1, 2, 3]))];
        let result = isolated_validate(tools);
        assert!(result.is_empty(), "array schema should be rejected");
    }

    #[test]
    fn rejects_non_object_schema_null() {
        let tools = vec![make_tool("bad_null", json!(null))];
        let result = isolated_validate(tools);
        assert!(result.is_empty(), "null schema should be rejected");
    }

    // ── Rule 2: must have "type" key ─────────────────────────────────────────

    #[test]
    fn rejects_object_without_type_field() {
        let tools = vec![make_tool(
            "no_type",
            json!({ "properties": { "x": { "type": "string" } } }),
        )];
        let result = isolated_validate(tools);
        assert!(result.is_empty(), "schema without 'type' should be rejected");
    }

    // ── Rule 3: required ⊆ properties ────────────────────────────────────────

    #[test]
    fn rejects_required_field_missing_from_properties() {
        let tools = vec![make_tool(
            "bad_required",
            json!({
                "type": "object",
                "properties": {},
                "required": ["missing_field"]
            }),
        )];
        let result = isolated_validate(tools);
        assert!(result.is_empty(), "schema with required field not in properties should be rejected");
    }

    #[test]
    fn accepts_valid_schema_with_required() {
        let tools = vec![make_tool("file_read", valid_schema())];
        let result = isolated_validate(tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "file_read");
    }

    // ── Edge cases ────────────────────────────────────────────────────────────

    #[test]
    fn accepts_schema_with_no_required_field() {
        let tools = vec![make_tool(
            "no_required",
            json!({ "type": "object", "properties": {} }),
        )];
        let result = isolated_validate(tools);
        assert_eq!(result.len(), 1, "schema without 'required' is valid");
    }

    #[test]
    fn accepts_schema_with_empty_required_array() {
        let tools = vec![make_tool(
            "empty_required",
            json!({ "type": "object", "properties": {}, "required": [] }),
        )];
        let result = isolated_validate(tools);
        assert_eq!(result.len(), 1, "empty required array is valid");
    }

    #[test]
    fn mixed_valid_and_invalid_tools_filtered() {
        let tools = vec![
            make_tool("good", valid_schema()),
            make_tool("bad", json!("string_not_object")),
            make_tool(
                "good2",
                json!({ "type": "object", "properties": { "x": { "type": "integer" } }, "required": ["x"] }),
            ),
        ];
        let result = isolated_validate(tools);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "good");
        assert_eq!(result[1].name, "good2");
    }

    // ── Cache behaviour ───────────────────────────────────────────────────────

    #[test]
    fn cache_populated_after_first_validate() {
        // Use a unique name + cache_contains rather than asserting total cache_size,
        // which is non-deterministic when parallel tests share the global cache.
        // Never call clear_cache() here — it would invalidate other concurrently
        // running tests' entries, causing non-deterministic failures at --test-threads=16.
        let _ = preflight_validate(vec![make_tool("cached_tool_unique_abc", valid_schema())]);
        assert!(
            cache_contains("cached_tool_unique_abc"),
            "valid tool should be cached after first call"
        );
    }

    #[test]
    fn cache_not_populated_for_invalid_tool() {
        // Use cache_contains instead of cache_size — parallel tests may add other
        // valid tools concurrently, making a total-size assertion non-deterministic.
        // Never call clear_cache() — it would disturb concurrent test entries.
        let tools = vec![make_tool("invalid_tool_xyz_unique", json!("not an object"))];
        let _ = preflight_validate(tools);
        assert!(
            !cache_contains("invalid_tool_xyz_unique"),
            "invalid tool must NOT be cached"
        );
    }

    #[test]
    fn second_call_same_tool_uses_cache() {
        // Use a globally-unique name so concurrent tests never collide.
        // Avoid clear_cache() and cache_size() — both are non-deterministic under
        // --test-threads=16 because they operate on the shared global cache.
        let unique_name = "cached_tool2_solo_kx8";
        let schema = valid_schema();
        // First call — populates cache.
        let _ = preflight_validate(vec![make_tool(unique_name, schema.clone())]);
        assert!(cache_contains(unique_name), "tool must be cached after first call");
        // Second call — cache hit; tool still passes through validation.
        let result = preflight_validate(vec![make_tool(unique_name, schema)]);
        assert_eq!(result.len(), 1, "tool must still pass through on cache hit");
        assert!(cache_contains(unique_name), "entry must persist after cache hit");
    }

    #[test]
    fn empty_input_returns_empty_output() {
        let result = isolated_validate(vec![]);
        assert!(result.is_empty());
    }

    // ── Concurrent cache safety ────────────────────────────────────────────────

    #[test]
    fn cache_concurrent_no_cross_contamination() {
        // Verify that parallel preflight_validate calls with unique tool names
        // do not corrupt each other's return values under --test-threads=16.
        //
        // IMPORTANT: We check the *return value* of preflight_validate (captured
        // atomically inside each thread) rather than post-hoc cache_contains().
        // Post-hoc cache checks are racy: isolated_validate() (used by other tests
        // running concurrently) calls clear_cache() which can evict entries between
        // the spawn/join and the assertion.  Return values are immune to this race.
        let handles: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    let name = format!("concurrent_unique_tool_h1_{i}");
                    let result = preflight_validate(vec![make_tool(&name, valid_schema())]);
                    (name, result.len())
                })
            })
            .collect();

        for handle in handles {
            let (name, len) = handle.join().unwrap();
            assert_eq!(
                len, 1,
                "concurrent thread for tool '{name}' must get a non-empty result (no cross-contamination)"
            );
        }
    }
}
