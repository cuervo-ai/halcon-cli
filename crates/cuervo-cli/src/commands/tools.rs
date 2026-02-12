//! `cuervo tools` — tool diagnostics, health checks, and validation.
//!
//! Sub-commands:
//! - `doctor`: run health checks on all registered tools
//! - `list`: show all available tools with permission levels
//! - `validate`: check tool schemas and configuration
//!
//! Read-only: no side effects.

use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use cuervo_core::types::{AppConfig, PermissionLevel, ToolInput};
use cuervo_storage::Database;
use cuervo_tools::background::ProcessRegistry;

use crate::render::{components, theme};

/// Run the `tools doctor` diagnostic: instantiate every tool, validate schema,
/// dry-run local tools, report health.
pub async fn doctor(config: &AppConfig, db_path: Option<&Path>) -> Result<()> {
    let t = theme::active();
    let r = theme::reset();
    let mut out = io::stderr().lock();

    let primary = t.palette.primary.fg();
    let h = crate::render::color::box_horiz();
    let tl = crate::render::color::box_top_left();
    let tr = crate::render::color::box_top_right();
    let bl = crate::render::color::box_bottom_left();
    let br = crate::render::color::box_bottom_right();
    let muted = t.palette.muted.fg();

    let _ = writeln!(
        out,
        "\n  {muted}{tl}{h} {primary}Tools Doctor{r} {muted}{}{tr}{r}",
        h.repeat(39),
    );

    // --- 1. Registry Health ---
    let proc_reg = Arc::new(ProcessRegistry::new(5));
    let registry = cuervo_tools::full_registry(&config.tools, Some(proc_reg));
    let mut defs = registry.tool_definitions();
    defs.sort_by(|a, b| a.name.cmp(&b.name));

    let tool_count = defs.len();

    components::section_header("Registry Health", &mut out);
    let success = t.palette.success.fg();
    let _ = writeln!(out, "    Registered tools: {success}{tool_count}{r}");

    // Categorize by permission level.
    let mut readonly_count = 0;
    let mut readwrite_count = 0;
    let mut destructive_count = 0;
    let mut tools_sorted: Vec<(&str, PermissionLevel, bool)> = Vec::new();

    let dummy_input = ToolInput {
        tool_use_id: "doctor-probe".into(),
        arguments: serde_json::json!({}),
        working_directory: "/tmp".into(),
    };

    for def in &defs {
        if let Some(tool) = registry.get(&def.name) {
            let perm = tool.permission_level();
            let confirms = tool.requires_confirmation(&dummy_input);
            match perm {
                PermissionLevel::ReadOnly => readonly_count += 1,
                PermissionLevel::ReadWrite => readwrite_count += 1,
                PermissionLevel::Destructive => destructive_count += 1,
            }
            tools_sorted.push((tool.name(), perm, confirms));
        }
    }

    let dim = t.palette.text_dim.fg();
    let _ = writeln!(
        out,
        "    {dim}ReadOnly: {readonly_count}  |  ReadWrite: {readwrite_count}  |  Destructive: {destructive_count}{r}"
    );

    // --- 2. Schema Validation ---
    components::section_header("Schema Validation", &mut out);
    let mut schema_pass = 0;
    let mut schema_fail = 0;
    let mut schema_errors: Vec<(String, String)> = Vec::new();

    for def in &defs {
        let schema = &def.input_schema;
        let mut errors: Vec<String> = Vec::new();

        if schema["type"] != "object" {
            errors.push("type is not 'object'".into());
        }
        if !schema["properties"].is_object() {
            errors.push("missing 'properties' object".into());
        }
        if !schema["required"].is_array() {
            errors.push("missing 'required' array".into());
        }

        // Validate required fields exist in properties.
        if let (Some(props), Some(required)) =
            (schema["properties"].as_object(), schema["required"].as_array())
        {
            for req in required {
                if let Some(field) = req.as_str() {
                    if !props.contains_key(field) {
                        errors.push(format!("required field '{field}' not in properties"));
                    }
                }
            }
        }

        if errors.is_empty() {
            schema_pass += 1;
        } else {
            schema_fail += 1;
            for err in errors {
                schema_errors.push((def.name.clone(), err));
            }
        }
    }

    if schema_fail == 0 {
        let badge = components::badge("PASS", components::BadgeLevel::Success);
        let _ = writeln!(
            out,
            "    {badge} All {schema_pass} schemas valid"
        );
    } else {
        let badge = components::badge("FAIL", components::BadgeLevel::Error);
        let _ = writeln!(
            out,
            "    {badge} {schema_pass} passed, {schema_fail} failed"
        );
        for (name, err) in &schema_errors {
            let _ = writeln!(out, "      {name}: {err}");
        }
    }

    // --- 3. Tool-by-Tool Listing ---
    components::section_header("Tool Inventory", &mut out);
    let accent = t.palette.accent.fg();

    for (name, perm, confirms) in &tools_sorted {
        let perm_str = match perm {
            PermissionLevel::ReadOnly => "RO",
            PermissionLevel::ReadWrite => "RW",
            PermissionLevel::Destructive => "D!",
        };
        let perm_badge = match perm {
            PermissionLevel::ReadOnly => {
                components::badge(perm_str, components::BadgeLevel::Success)
            }
            PermissionLevel::ReadWrite => {
                components::badge(perm_str, components::BadgeLevel::Warning)
            }
            PermissionLevel::Destructive => {
                components::badge(perm_str, components::BadgeLevel::Error)
            }
        };
        let confirm_indicator = if *confirms {
            format!("{dim} (confirms){r}")
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "    {accent}{name:<22}{r} {perm_badge}{confirm_indicator}"
        );
    }

    // --- 4. Probe ReadOnly Tools ---
    components::section_header("ReadOnly Tool Probes", &mut out);
    let probeable = [
        ("glob", serde_json::json!({"pattern": "*.NONEXISTENT_DOCTOR_PROBE"})),
        ("grep", serde_json::json!({"pattern": "NONEXISTENT_DOCTOR_PROBE_xyz123"})),
        ("directory_tree", serde_json::json!({"path": "/tmp", "depth": 1})),
        ("task_track", serde_json::json!({"action": "list"})),
    ];

    let mut probe_pass = 0;
    let mut probe_fail = 0;

    for (tool_name, args) in &probeable {
        if let Some(tool) = registry.get(tool_name) {
            let input = ToolInput {
                tool_use_id: format!("doctor-probe-{tool_name}"),
                arguments: args.clone(),
                working_directory: "/tmp".into(),
            };
            let start = std::time::Instant::now();
            match tool.execute(input).await {
                Ok(output) => {
                    let elapsed = start.elapsed().as_millis();
                    if output.is_error {
                        probe_fail += 1;
                        let badge = components::badge("FAIL", components::BadgeLevel::Error);
                        let _ = writeln!(
                            out,
                            "    {tool_name:<20} {badge} {dim}({elapsed}ms) error: {}{r}",
                            truncate_str(&output.content, 60),
                        );
                    } else {
                        probe_pass += 1;
                        let badge = components::badge("OK", components::BadgeLevel::Success);
                        let _ = writeln!(
                            out,
                            "    {tool_name:<20} {badge} {dim}({elapsed}ms){r}"
                        );
                    }
                }
                Err(e) => {
                    probe_fail += 1;
                    let badge = components::badge("ERR", components::BadgeLevel::Error);
                    let _ = writeln!(
                        out,
                        "    {tool_name:<20} {badge} {dim}{}{r}",
                        truncate_str(&e.to_string(), 60),
                    );
                }
            }
        }
    }

    let _ = writeln!(out, "    {dim}---{r}");
    let total_probes = probe_pass + probe_fail;
    if probe_fail == 0 {
        let badge = components::badge("PASS", components::BadgeLevel::Success);
        let _ = writeln!(
            out,
            "    {badge} {probe_pass}/{total_probes} probes succeeded"
        );
    } else {
        let badge = components::badge("WARN", components::BadgeLevel::Warning);
        let _ = writeln!(
            out,
            "    {badge} {probe_pass}/{total_probes} probes succeeded, {probe_fail} failed"
        );
    }

    // --- 5. Configuration Checks ---
    components::section_header("Configuration", &mut out);

    let timeout_badge = if config.tools.timeout_secs > 0 && config.tools.timeout_secs <= 300 {
        components::badge("OK", components::BadgeLevel::Success)
    } else if config.tools.timeout_secs == 0 {
        components::badge("WARN", components::BadgeLevel::Warning)
    } else {
        components::badge("HIGH", components::BadgeLevel::Warning)
    };
    let _ = writeln!(
        out,
        "    Timeout:       {timeout_badge} {dim}{}s{r}",
        config.tools.timeout_secs,
    );

    let sandbox_badge = if config.tools.sandbox.enabled {
        components::badge("ON", components::BadgeLevel::Success)
    } else {
        components::badge("OFF", components::BadgeLevel::Muted)
    };
    let _ = writeln!(out, "    Sandbox:       {sandbox_badge}");

    let dryrun_badge = if config.tools.dry_run {
        components::badge("ON", components::BadgeLevel::Success)
    } else {
        components::badge("OFF", components::BadgeLevel::Muted)
    };
    let _ = writeln!(out, "    Dry-run:       {dryrun_badge}");

    let confirm_badge = if config.tools.confirm_destructive {
        components::badge("ON", components::BadgeLevel::Success)
    } else {
        components::badge("WARN", components::BadgeLevel::Warning)
    };
    let _ = writeln!(out, "    Confirm destr: {confirm_badge}");

    if !config.tools.allowed_directories.is_empty() {
        let _ = writeln!(
            out,
            "    {dim}Allowed dirs: {}{r}",
            config.tools.allowed_directories.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        );
    }
    if !config.tools.blocked_patterns.is_empty() {
        let _ = writeln!(
            out,
            "    {dim}Blocked patterns: {}{r}",
            config.tools.blocked_patterns.join(", ")
        );
    }

    // --- 6. Execution Metrics (from DB) ---
    let db = open_db(config, db_path)?;
    if let Some(db) = &db {
        components::section_header("Execution Metrics", &mut out);
        match db.top_tool_stats(10) {
            Ok(stats) if !stats.is_empty() => {
                for stat in &stats {
                    let success_pct = (stat.success_rate * 100.0).round() as u32;
                    let (badge_text, level) = if success_pct >= 95 {
                        ("OK", components::BadgeLevel::Success)
                    } else if success_pct >= 80 {
                        ("WARN", components::BadgeLevel::Warning)
                    } else {
                        ("FAIL", components::BadgeLevel::Error)
                    };
                    let badge = components::badge(badge_text, level);
                    let _ = writeln!(
                        out,
                        "    {accent}{:<20}{r} {badge} {dim}{} calls, {:.0}ms avg, {}% success{r}",
                        stat.tool_name,
                        stat.total_executions,
                        stat.avg_duration_ms,
                        success_pct,
                    );
                }
            }
            Ok(_) => {
                let _ = writeln!(out, "    {muted}(no tool execution data yet){r}");
            }
            Err(_) => {
                let _ = writeln!(out, "    {muted}(tool metrics unavailable){r}");
            }
        }
    }

    // --- 7. Recommendations ---
    components::section_header("Recommendations", &mut out);
    let mut recs: Vec<String> = Vec::new();

    if !config.tools.confirm_destructive {
        recs.push("Enable destructive tool confirmation: tools.confirm_destructive = true".into());
    }
    if config.tools.timeout_secs == 0 {
        recs.push("Set a tool timeout to prevent runaway commands: tools.timeout_secs = 30".into());
    }
    if config.tools.timeout_secs > 300 {
        recs.push(format!(
            "Tool timeout is {}s (>5min) — consider lowering to prevent hangs",
            config.tools.timeout_secs
        ));
    }
    if !config.tools.sandbox.enabled {
        recs.push("Consider enabling sandbox for bash tool: tools.sandbox.enabled = true".into());
    }
    if schema_fail > 0 {
        recs.push(format!("{schema_fail} tool(s) have invalid schemas — check implementation"));
    }
    if probe_fail > 0 {
        recs.push(format!("{probe_fail} ReadOnly probe(s) failed — investigate tool health"));
    }

    // Check DB for low-success-rate tools.
    if let Some(db) = &db {
        if let Ok(stats) = db.top_tool_stats(23) {
            for stat in &stats {
                if stat.success_rate < 0.80 && stat.total_executions >= 5 {
                    recs.push(format!(
                        "{}: low success rate ({:.0}%) across {} executions",
                        stat.tool_name,
                        stat.success_rate * 100.0,
                        stat.total_executions,
                    ));
                }
            }
        }
    }

    if recs.is_empty() {
        let _ = writeln!(out, "    {success}All tool systems nominal{r}");
    } else {
        let warn = t.palette.warning.fg();
        for rec in &recs {
            let _ = writeln!(out, "    {warn}-{r} {rec}");
        }
    }

    // --- Summary ---
    let _ = writeln!(out);
    let total_checks = schema_pass + schema_fail + total_probes;
    let total_pass = schema_pass + probe_pass;
    let total_fail = schema_fail + probe_fail;

    if total_fail == 0 {
        let badge = components::badge("HEALTHY", components::BadgeLevel::Success);
        let _ = writeln!(
            out,
            "  {badge} {total_pass}/{total_checks} checks passed — all tools operational"
        );
    } else {
        let badge = components::badge("DEGRADED", components::BadgeLevel::Warning);
        let _ = writeln!(
            out,
            "  {badge} {total_pass}/{total_checks} passed, {total_fail} issues found"
        );
    }

    let _ = writeln!(
        out,
        "  {muted}{bl}{}{br}{r}\n",
        h.repeat(54),
    );

    let _ = out.flush();
    Ok(())
}

/// Run the `tools list` sub-command: show all tools with permission levels.
pub fn list(config: &AppConfig) -> Result<()> {
    let t = theme::active();
    let r = theme::reset();
    let mut out = io::stderr().lock();

    let proc_reg = Arc::new(ProcessRegistry::new(5));
    let registry = cuervo_tools::full_registry(&config.tools, Some(proc_reg));
    let mut defs = registry.tool_definitions();
    defs.sort_by(|a, b| a.name.cmp(&b.name));

    let accent = t.palette.accent.fg();
    let dim = t.palette.text_dim.fg();

    let _ = writeln!(out, "\n  {accent}Available Tools ({}):{r}\n", defs.len());

    let dummy_input = ToolInput {
        tool_use_id: "list".into(),
        arguments: serde_json::json!({}),
        working_directory: "/tmp".into(),
    };

    for def in &defs {
        if let Some(tool) = registry.get(&def.name) {
            let perm = tool.permission_level();
            let perm_str = match perm {
                PermissionLevel::ReadOnly => "RO",
                PermissionLevel::ReadWrite => "RW",
                PermissionLevel::Destructive => "D!",
            };
            let perm_badge = match perm {
                PermissionLevel::ReadOnly => {
                    components::badge(perm_str, components::BadgeLevel::Success)
                }
                PermissionLevel::ReadWrite => {
                    components::badge(perm_str, components::BadgeLevel::Warning)
                }
                PermissionLevel::Destructive => {
                    components::badge(perm_str, components::BadgeLevel::Error)
                }
            };
            let desc = truncate_str(tool.description(), 50);
            let confirms = if tool.requires_confirmation(&dummy_input) {
                " *"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "  {accent}{:<22}{r} {perm_badge}{confirms}  {dim}{desc}{r}",
                tool.name(),
            );
        }
    }

    let _ = writeln!(out, "\n  {dim}* = requires confirmation{r}\n");
    let _ = out.flush();
    Ok(())
}

/// Run the `tools validate` sub-command: deep schema validation.
pub fn validate(config: &AppConfig) -> Result<()> {
    let t = theme::active();
    let r = theme::reset();
    let mut out = io::stderr().lock();

    let proc_reg = Arc::new(ProcessRegistry::new(5));
    let registry = cuervo_tools::full_registry(&config.tools, Some(proc_reg));
    let mut defs = registry.tool_definitions();
    defs.sort_by(|a, b| a.name.cmp(&b.name));

    let _ = writeln!(out, "\n  Schema Validation Report\n");

    let mut pass = 0;
    let mut fail = 0;
    let dim = t.palette.text_dim.fg();

    for def in &defs {
        let schema = &def.input_schema;
        let mut errors: Vec<String> = Vec::new();

        // Basic structure checks.
        if schema["type"] != "object" {
            errors.push("type is not 'object'".into());
        }
        if !schema["properties"].is_object() {
            errors.push("missing 'properties'".into());
        }
        if !schema["required"].is_array() {
            errors.push("missing 'required' array".into());
        }

        // Cross-reference required ↔ properties.
        if let (Some(props), Some(required)) =
            (schema["properties"].as_object(), schema["required"].as_array())
        {
            for req in required {
                if let Some(field) = req.as_str() {
                    if !props.contains_key(field) {
                        errors.push(format!("required '{field}' not in properties"));
                    }
                }
            }

            // Check each property has a type.
            for (key, val) in props {
                if val.get("type").is_none() && val.get("enum").is_none() {
                    errors.push(format!("property '{key}' has no type or enum"));
                }
            }
        }

        // Name/description checks.
        if def.name.is_empty() {
            errors.push("empty tool name".into());
        }
        if def.description.is_empty() {
            errors.push("empty description".into());
        }

        if errors.is_empty() {
            pass += 1;
            let badge = components::badge("OK", components::BadgeLevel::Success);
            let _ = writeln!(out, "  {badge} {}", def.name);
        } else {
            fail += 1;
            let badge = components::badge("FAIL", components::BadgeLevel::Error);
            let _ = writeln!(out, "  {badge} {}", def.name);
            for err in &errors {
                let _ = writeln!(out, "    {dim}- {err}{r}");
            }
        }
    }

    let _ = writeln!(out);
    if fail == 0 {
        let badge = components::badge("PASS", components::BadgeLevel::Success);
        let _ = writeln!(out, "  {badge} All {pass} tools passed validation\n");
    } else {
        let badge = components::badge("FAIL", components::BadgeLevel::Error);
        let _ = writeln!(
            out,
            "  {badge} {pass} passed, {fail} failed validation\n"
        );
    }

    let _ = out.flush();
    Ok(())
}

// --- Helpers ---

fn open_db(
    config: &AppConfig,
    db_path: Option<&Path>,
) -> Result<Option<Arc<Database>>> {
    let path = db_path
        .map(|p| p.to_path_buf())
        .or_else(|| config.storage.database_path.clone())
        .or_else(|| dirs::home_dir().map(|h| h.join(".cuervo").join("cuervo.db")));

    match path {
        Some(p) if p.exists() => {
            let db = Database::open(&p)?;
            Ok(Some(Arc::new(db)))
        }
        _ => Ok(None),
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::types::AppConfig;

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn truncate_str_long() {
        let result = truncate_str("hello world this is long", 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 10);
    }

    #[test]
    fn list_runs_without_crash() {
        let config = AppConfig::default();
        let result = list(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_runs_without_crash() {
        let config = AppConfig::default();
        let result = validate(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn doctor_runs_without_crash() {
        let config = AppConfig::default();
        let result = doctor(&config, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn doctor_with_in_memory_db() {
        let config = AppConfig::default();
        // doctor opens DB via config path, which won't exist in test.
        // Just ensure it doesn't crash with None path.
        let result = doctor(&config, None).await;
        assert!(result.is_ok());
    }

    #[test]
    fn registry_builds_with_default_config() {
        let config = AppConfig::default();
        let proc_reg = Arc::new(ProcessRegistry::new(5));
        let registry = cuervo_tools::full_registry(&config.tools, Some(proc_reg));
        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 23);
    }

    #[test]
    fn all_tools_have_valid_schemas() {
        let config = AppConfig::default();
        let proc_reg = Arc::new(ProcessRegistry::new(5));
        let registry = cuervo_tools::full_registry(&config.tools, Some(proc_reg));
        for def in registry.tool_definitions() {
            let schema = &def.input_schema;
            assert_eq!(schema["type"], "object", "{} missing type", def.name);
            assert!(
                schema["properties"].is_object(),
                "{} missing properties",
                def.name
            );
            assert!(
                schema["required"].is_array(),
                "{} missing required",
                def.name
            );
        }
    }

    #[test]
    fn all_required_fields_exist_in_properties() {
        let config = AppConfig::default();
        let proc_reg = Arc::new(ProcessRegistry::new(5));
        let registry = cuervo_tools::full_registry(&config.tools, Some(proc_reg));
        for def in registry.tool_definitions() {
            let schema = &def.input_schema;
            if let (Some(props), Some(required)) =
                (schema["properties"].as_object(), schema["required"].as_array())
            {
                for req in required {
                    if let Some(field) = req.as_str() {
                        assert!(
                            props.contains_key(field),
                            "{}: required field '{}' not in properties",
                            def.name,
                            field
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn permission_levels_cover_all_tools() {
        let config = AppConfig::default();
        let proc_reg = Arc::new(ProcessRegistry::new(5));
        let registry = cuervo_tools::full_registry(&config.tools, Some(proc_reg));
        for def in registry.tool_definitions() {
            let tool = registry.get(&def.name).unwrap();
            // Should not panic — every tool has a valid permission level.
            let _ = tool.permission_level();
        }
    }

    #[test]
    fn destructive_tools_require_confirmation() {
        let config = AppConfig::default();
        let proc_reg = Arc::new(ProcessRegistry::new(5));
        let registry = cuervo_tools::full_registry(&config.tools, Some(proc_reg));
        let dummy = ToolInput {
            tool_use_id: "test".into(),
            arguments: serde_json::json!({}),
            working_directory: "/tmp".into(),
        };
        for def in registry.tool_definitions() {
            let tool = registry.get(&def.name).unwrap();
            if tool.permission_level() == PermissionLevel::Destructive {
                // All destructive tools should require confirmation
                // (except background_kill which is agent-initiated).
                if tool.name() != "background_kill" {
                    assert!(
                        tool.requires_confirmation(&dummy),
                        "{} is Destructive but doesn't require confirmation",
                        tool.name()
                    );
                }
            }
        }
    }

    #[test]
    fn readonly_tools_never_require_confirmation() {
        let config = AppConfig::default();
        let proc_reg = Arc::new(ProcessRegistry::new(5));
        let registry = cuervo_tools::full_registry(&config.tools, Some(proc_reg));
        let dummy = ToolInput {
            tool_use_id: "test".into(),
            arguments: serde_json::json!({}),
            working_directory: "/tmp".into(),
        };
        for def in registry.tool_definitions() {
            let tool = registry.get(&def.name).unwrap();
            if tool.permission_level() == PermissionLevel::ReadOnly {
                assert!(
                    !tool.requires_confirmation(&dummy),
                    "{} is ReadOnly but requires confirmation",
                    tool.name()
                );
            }
        }
    }

    #[tokio::test]
    async fn probe_glob_returns_ok() {
        let config = AppConfig::default();
        let registry = cuervo_tools::default_registry(&config.tools);
        let tool = registry.get("glob").unwrap();
        let input = ToolInput {
            tool_use_id: "probe".into(),
            arguments: serde_json::json!({"pattern": "*.NONEXISTENT_xyz"}),
            working_directory: "/tmp".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_error);
    }

    #[tokio::test]
    async fn probe_task_track_list_returns_ok() {
        let config = AppConfig::default();
        let registry = cuervo_tools::default_registry(&config.tools);
        let tool = registry.get("task_track").unwrap();
        let input = ToolInput {
            tool_use_id: "probe".into(),
            arguments: serde_json::json!({"action": "list"}),
            working_directory: "/tmp".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_error);
    }

    #[tokio::test]
    async fn probe_directory_tree_returns_ok() {
        let config = AppConfig::default();
        let registry = cuervo_tools::default_registry(&config.tools);
        let tool = registry.get("directory_tree").unwrap();
        let input = ToolInput {
            tool_use_id: "probe".into(),
            arguments: serde_json::json!({"path": "/tmp", "depth": 1}),
            working_directory: "/tmp".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_error);
    }
}
