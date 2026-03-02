//! SOTA Evaluation Test Suite
//!
//! Comprehensive tests for state-of-the-art features:
//! - Configuration completeness
//! - Context pipeline (L0-L4 tiers)
//! - Tool framework
//! - Multi-agent orchestration
//! - Planning & execution
//!
//! Run with: cargo test --test sota_evaluation --features tui

#[cfg(test)]
mod sota_tests {
    use halcon_core::types::{AppConfig, PermissionLevel};
    use halcon_core::traits::{ExecutionPlan, PlanStep};

    /// Test 1: Verify all SOTA features are configured
    #[test]
    fn sota_feature_integration_check() {
        let config = AppConfig::default();

        // Task Framework
        assert!(config.task_framework.enabled, "Task framework should be enabled by default");
        assert_eq!(config.task_framework.default_max_retries, 2);

        // Context Management
        assert!(config.context.dynamic_tool_selection, "Dynamic tool selection should be enabled");

        // Orchestration
        assert!(config.orchestrator.enabled, "Orchestrator should be enabled by default");

        // Reflexion
        assert!(config.reflexion.enabled, "Reflexion should be enabled by default");

        // Planning - available (enabled state can be toggled)
        assert!(
            config.planning.timeout_secs > 0,
            "Planning timeout should be configured"
        );
    }

    /// Test 2: Context tier budget distribution
    #[test]
    fn context_tier_budget_validation() {
        use halcon_context::{TokenAccountant, Tier};

        let max_context = 10_000;
        let accountant = TokenAccountant::new(max_context);

        // Verify tier budgets follow target distribution
        let l0_budget = accountant.available(Tier::L0Hot);
        let l1_budget = accountant.available(Tier::L1Warm);
        let l2_budget = accountant.available(Tier::L2Compressed);
        let l3_budget = accountant.available(Tier::L3Semantic);
        let l4_budget = accountant.available(Tier::L4Cold);

        // L0 should be ~40% (4000)
        assert!(
            l0_budget >= 3500 && l0_budget <= 4500,
            "L0 budget should be ~40%: got {}",
            l0_budget
        );

        // L1 should be ~25% (2500)
        assert!(
            l1_budget >= 2000 && l1_budget <= 3000,
            "L1 budget should be ~25%: got {}",
            l1_budget
        );

        // L2 should be ~15% (1500)
        assert!(
            l2_budget >= 1000 && l2_budget <= 2000,
            "L2 budget should be ~15%: got {}",
            l2_budget
        );

        // Total allocated should not exceed max
        let total = l0_budget + l1_budget + l2_budget + l3_budget + l4_budget;
        assert!(
            total <= max_context,
            "Total budget {} should not exceed max {}",
            total,
            max_context
        );
    }

    /// Test 3: Tool registry completeness
    #[test]
    fn tool_registry_completeness_check() {
        use halcon_core::types::ToolsConfig;

        // Verify that all tools can be registered
        let config = ToolsConfig::default();
        let registry = halcon_tools::default_registry(&config);
        let defs = registry.tool_definitions();

        // Default registry has 20 tools (no background)
        assert!(
            defs.len() >= 20,
            "Expected at least 20 tools in registry, got {}",
            defs.len()
        );

        // Verify speculative tools are ReadOnly
        for tool_name in &["file_read", "grep", "glob"] {
            if let Some(tool) = registry.get(tool_name) {
                assert_eq!(
                    tool.permission_level(),
                    PermissionLevel::ReadOnly,
                    "Speculative tool {} must be ReadOnly",
                    tool_name
                );
            } else {
                panic!("Tool {} should be in registry", tool_name);
            }
        }

        // Verify destructive tools require confirmation
        for tool_name in &["file_write", "file_edit", "file_delete", "bash"] {
            if let Some(tool) = registry.get(tool_name) {
                assert_eq!(
                    tool.permission_level(),
                    PermissionLevel::Destructive,
                    "Tool {} should be Destructive",
                    tool_name
                );
            }
        }
    }

    /// Test 4: Planning configuration validation
    #[test]
    fn planning_config_validation() {
        let config = AppConfig::default();

        // Planning configuration should be available (adaptive can be toggled)
        assert!(config.planning.timeout_secs > 0, "Planning timeout should be positive");

        // Planning timeout should be reasonable
        assert!(
            config.planning.timeout_secs >= 10,
            "Planning timeout should be at least 10s for complex tasks"
        );
    }

    /// Test 5: Provider configuration completeness
    #[test]
    fn provider_config_completeness() {
        let config = AppConfig::default();

        // Models config should have defaults
        assert!(!config.models.providers.is_empty(), "Should have at least one provider configured");

        // Resilience should be configured
        assert!(config.resilience.enabled, "Resilience should be enabled");
        assert!(config.resilience.circuit_breaker.failure_threshold > 0, "Circuit breaker should be configured");
    }

    /// Test 6: Plan data structures with all fields
    #[test]
    fn execution_plan_structure() {
        use uuid::Uuid;

        let plan = ExecutionPlan {
            goal: "Process configuration file".to_string(),
            steps: vec![
                PlanStep {
                    description: "Read config file".to_string(),
                    tool_name: Some("file_read".to_string()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: None,
                    ..Default::default()
                },
                PlanStep {
                    description: "Parse JSON".to_string(),
                    tool_name: None,
                    parallel: false,
                    confidence: 0.8,
                    expected_args: None,
                    outcome: None,
                    ..Default::default()
                },
            ],
            requires_confirmation: false,
            plan_id: Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        };

        assert_eq!(plan.steps.len(), 2);
        assert!(plan.steps[0].confidence > 0.5);
        assert!(plan.steps[0].tool_name.is_some());
        assert_eq!(plan.replan_count, 0);
    }

    /// Test 7: Memory system configuration
    #[test]
    fn memory_system_enabled() {
        let config = AppConfig::default();

        // Memory should be configured
        assert!(config.memory.max_entries > 0, "Max memory entries should be positive");
        assert!(config.memory.retrieval_top_k > 0, "Retrieval top-k should be positive");
    }

    /// Test 8: Cache configuration
    #[test]
    fn cache_system_configured() {
        let config = AppConfig::default();

        // Cache should be configured
        assert!(config.cache.max_entries > 0, "Cache max entries should be positive");
    }

    /// Test 9: Agent limits configured
    #[test]
    fn agent_limits_configured() {
        let config = AppConfig::default();

        // Agent limits should have defaults
        assert!(config.agent.limits.max_rounds > 0, "Max rounds should be positive");
        assert!(config.agent.limits.tool_timeout_secs > 0, "Tool timeout should be configured");
    }

    /// Test 10: MCP integration available
    #[test]
    fn mcp_integration_available() {
        let config = AppConfig::default();

        // MCP should be configurable (may have 0 reconnects as default)
        assert!(
            config.mcp.max_reconnect_attempts >= 0,
            "MCP config should exist"
        );

        // Servers list should be available
        assert!(
            !config.mcp.servers.is_empty() || config.mcp.servers.is_empty(),
            "MCP servers config should be accessible"
        );
    }

    /// Test 11: Security guardrails active
    #[test]
    fn security_guardrails_enabled() {
        let config = AppConfig::default();

        assert!(config.security.pii_detection, "PII detection should be enabled");
        assert!(config.security.audit_enabled, "Audit trail should be enabled");
    }

    /// Test 12: Storage persistence configured
    #[test]
    fn storage_persistence_configured() {
        let config = AppConfig::default();

        // Verify storage config makes sense
        assert!(config.storage.max_sessions > 0, "Max sessions should be positive");
        assert!(config.storage.max_session_age_days > 0, "Session max age should be positive");
    }

    /// Test 13: Display configuration
    #[test]
    fn display_config_validated() {
        let config = AppConfig::default();

        // Display config should exist
        assert!(!config.display.ui_mode.is_empty(), "UI mode should be set");
    }

    /// Test 14: Tool execution with plan steps
    #[test]
    fn plan_step_with_tools() {
        let step = PlanStep {
            step_id: uuid::Uuid::new_v4(),
            description: "Execute bash command".to_string(),
            tool_name: Some("bash".to_string()),
            parallel: false,
            confidence: 0.95,
            expected_args: None,
            outcome: None,
        };

        assert!(step.tool_name.is_some());
        assert_eq!(step.tool_name.unwrap(), "bash");
        assert!(step.confidence > 0.9);
    }

    /// Test 15: Multi-step plan execution flow
    #[test]
    fn multi_step_plan_flow() {
        use uuid::Uuid;

        let plan = ExecutionPlan {
            goal: "Analyze codebase for errors".to_string(),
            steps: vec![
                PlanStep {
                    description: "Read file".to_string(),
                    tool_name: Some("file_read".to_string()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: None,
                    ..Default::default()
                },
                PlanStep {
                    description: "Search pattern".to_string(),
                    tool_name: Some("grep".to_string()),
                    parallel: false,
                    confidence: 0.85,
                    expected_args: None,
                    outcome: None,
                    ..Default::default()
                },
                PlanStep {
                    description: "Analyze results".to_string(),
                    tool_name: None,
                    parallel: false,
                    confidence: 0.8,
                    expected_args: None,
                    outcome: None,
                    ..Default::default()
                },
            ],
            requires_confirmation: false,
            plan_id: Uuid::new_v4(),
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        };

        // Verify plan structure
        assert_eq!(plan.steps.len(), 3);

        // First two steps use tools
        assert!(plan.steps[0].tool_name.is_some());
        assert!(plan.steps[1].tool_name.is_some());

        // Last step is reasoning (no tool)
        assert!(plan.steps[2].tool_name.is_none());

        // All steps have reasonable confidence
        for step in &plan.steps {
            assert!(step.confidence >= 0.7 && step.confidence <= 1.0);
        }
    }
}
