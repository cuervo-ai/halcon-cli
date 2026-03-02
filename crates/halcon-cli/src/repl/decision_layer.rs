//! Decision Layer — Task Complexity Estimator for adaptive orchestration.
//!
//! Classifies incoming tasks into complexity tiers to prevent over-orchestration:
//!
//! | Tier              | Orchestration | Plan depth | Example                    |
//! |-------------------|---------------|------------|----------------------------|
//! | SimpleExecution   | None          | 1-2 steps  | "read this file"           |
//! | StructuredTask    | Optional      | 3-5 steps  | "find and fix this bug"    |
//! | MultiDomain       | Recommended   | 5-8 steps  | "refactor auth + add tests"|
//! | LongHorizon       | Required      | 8+ steps   | "full project audit"       |
//!
//! This prevents simple queries from spawning unnecessary sub-agents and
//! reduces coordinator token overhead.

use halcon_core::types::ToolDefinition;

/// Task complexity tier — determines orchestration strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TaskComplexity {
    /// Single tool call or direct answer. No orchestration needed.
    SimpleExecution,
    /// Multi-step but single-domain. Optional orchestration.
    StructuredTask,
    /// Cross-domain work requiring parallel execution.
    MultiDomain,
    /// Extended investigation requiring deep planning.
    LongHorizon,
}

/// Orchestration recommendation based on complexity analysis.
#[derive(Debug, Clone)]
pub(crate) struct OrchestrationDecision {
    pub complexity: TaskComplexity,
    pub use_orchestration: bool,
    pub recommended_max_rounds: u32,
    pub recommended_plan_depth: u32,
    pub reason: &'static str,
}

/// Signals used to estimate task complexity.
#[derive(Debug)]
pub(crate) struct TaskSignals {
    /// Word count of the user message.
    pub word_count: usize,
    /// Number of distinct action verbs detected.
    pub action_count: usize,
    /// Number of distinct domains (file, git, web, test, etc.) referenced.
    pub domain_count: usize,
    /// Whether the task mentions multiple files or components.
    pub multi_target: bool,
    /// Whether the task requires investigation/analysis before action.
    pub requires_investigation: bool,
    /// Number of available tools.
    pub tool_count: usize,
}

/// Action verb indicators for complexity estimation.
const ACTION_VERBS: &[&str] = &[
    "create", "write", "add", "implement", "build", "make",
    "fix", "repair", "debug", "patch", "resolve",
    "refactor", "restructure", "redesign", "rewrite",
    "delete", "remove", "clean", "purge",
    "test", "verify", "validate", "check", "audit",
    "deploy", "publish", "release", "ship",
    "analyze", "investigate", "review", "inspect", "explore",
    "configure", "setup", "install",
    // Spanish
    "crear", "escribir", "agregar", "implementar", "construir",
    "arreglar", "reparar", "depurar", "corregir",
    "refactorizar", "reescribir",
    "eliminar", "limpiar",
    "probar", "verificar", "validar", "revisar", "auditar",
    "desplegar", "publicar",
    "analizar", "investigar", "explorar",
    "configurar", "instalar",
];

/// Domain indicators — each match counts as a separate domain.
const DOMAIN_INDICATORS: &[(&str, &str)] = &[
    ("file", "file_ops"), ("read", "file_ops"), ("write", "file_ops"),
    ("edit", "file_ops"), ("create", "file_ops"),
    ("refactor", "code_ops"), ("restructure", "code_ops"), ("rewrite", "code_ops"),
    ("implement", "code_ops"), ("function", "code_ops"), ("module", "code_ops"),
    ("git", "git"), ("commit", "git"), ("branch", "git"), ("push", "git"),
    ("test", "testing"), ("spec", "testing"), ("coverage", "testing"),
    ("lint", "quality"), ("format", "quality"), ("style", "quality"),
    ("build", "build"), ("compile", "build"), ("cargo", "build"), ("npm", "build"),
    ("deploy", "deploy"), ("docker", "deploy"), ("ci", "deploy"),
    ("web", "web"), ("api", "web"), ("http", "web"), ("fetch", "web"),
    ("database", "data"), ("sql", "data"), ("migration", "data"),
    ("security", "security"), ("vulnerability", "security"), ("audit", "security"),
    // Spanish
    ("archivo", "file_ops"), ("leer", "file_ops"), ("escribir", "file_ops"),
    ("refactorizar", "code_ops"), ("módulo", "code_ops"),
    ("prueba", "testing"), ("cobertura", "testing"),
    ("construir", "build"), ("compilar", "build"),
    ("desplegar", "deploy"),
    ("seguridad", "security"), ("vulnerabilidad", "security"),
];

/// Multi-target indicators — suggests working across multiple files/components.
const MULTI_TARGET_INDICATORS: &[&str] = &[
    "all", "every", "each", "multiple", "several", "across", "throughout",
    "project", "codebase", "repository", "repo",
    "todos", "cada", "múltiples", "varios", "proyecto", "repositorio",
];

/// Investigation indicators — suggests analysis before action.
const INVESTIGATION_INDICATORS: &[&str] = &[
    "find", "search", "discover", "identify", "locate", "determine",
    "understand", "explain", "why", "how does", "how to", "what causes",
    "analyze", "investigate", "diagnose", "profile", "benchmark",
    "buscar", "encontrar", "identificar", "determinar",
    "entender", "explicar", "por qué", "cómo",
    "analizar", "investigar", "diagnosticar",
];

/// Estimate task complexity from user message and available context.
pub(crate) fn estimate_complexity(
    user_message: &str,
    available_tools: &[ToolDefinition],
) -> OrchestrationDecision {
    let signals = extract_signals(user_message, available_tools.len());
    classify(signals)
}

/// Extract complexity signals from the user message.
fn extract_signals(message: &str, tool_count: usize) -> TaskSignals {
    let lower = message.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let word_count = words.len();

    // Count distinct action verbs
    let action_count = ACTION_VERBS.iter()
        .filter(|verb| words.iter().any(|w| w.starts_with(**verb)))
        .count();

    // Count distinct domains
    let mut domains = std::collections::HashSet::new();
    for (keyword, domain) in DOMAIN_INDICATORS {
        if lower.contains(keyword) {
            domains.insert(*domain);
        }
    }
    let domain_count = domains.len();

    // Multi-target detection
    let multi_target = MULTI_TARGET_INDICATORS.iter()
        .any(|ind| lower.contains(ind));

    // Investigation detection
    let requires_investigation = INVESTIGATION_INDICATORS.iter()
        .any(|ind| lower.contains(ind));

    TaskSignals {
        word_count,
        action_count,
        domain_count,
        multi_target,
        requires_investigation,
        tool_count,
    }
}

/// Classify task signals into complexity tier and orchestration decision.
fn classify(signals: TaskSignals) -> OrchestrationDecision {
    // LongHorizon: many domains, many actions, or very long message
    if signals.domain_count >= 4
        || (signals.action_count >= 4 && signals.multi_target)
        || signals.word_count > 200
    {
        return OrchestrationDecision {
            complexity: TaskComplexity::LongHorizon,
            use_orchestration: true,
            recommended_max_rounds: 15,
            recommended_plan_depth: 8,
            reason: "long-horizon task: multiple domains or extensive scope",
        };
    }

    // MultiDomain: 2+ domains with multiple actions
    if signals.domain_count >= 2 && signals.action_count >= 2 {
        return OrchestrationDecision {
            complexity: TaskComplexity::MultiDomain,
            use_orchestration: true,
            recommended_max_rounds: 10,
            recommended_plan_depth: 6,
            reason: "multi-domain task: cross-cutting work recommended for parallel execution",
        };
    }

    // StructuredTask: single domain but non-trivial
    if signals.action_count >= 2
        || signals.requires_investigation
        || (signals.word_count > 30 && signals.domain_count >= 1)
    {
        return OrchestrationDecision {
            complexity: TaskComplexity::StructuredTask,
            use_orchestration: false,
            recommended_max_rounds: 8,
            recommended_plan_depth: 4,
            reason: "structured task: single-agent execution sufficient",
        };
    }

    // SimpleExecution: trivial task
    OrchestrationDecision {
        complexity: TaskComplexity::SimpleExecution,
        use_orchestration: false,
        recommended_max_rounds: 4,
        recommended_plan_depth: 2,
        reason: "simple execution: direct tool call or answer",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_tools() -> Vec<ToolDefinition> {
        vec![]
    }

    #[test]
    fn simple_file_read() {
        let d = estimate_complexity("read this file", &empty_tools());
        assert_eq!(d.complexity, TaskComplexity::SimpleExecution);
        assert!(!d.use_orchestration);
    }

    #[test]
    fn structured_bug_fix() {
        let d = estimate_complexity("find the bug in the login function and fix it", &empty_tools());
        assert!(d.complexity >= TaskComplexity::StructuredTask);
        assert!(!d.use_orchestration);
    }

    #[test]
    fn multi_domain_refactor_with_tests() {
        let d = estimate_complexity(
            "refactor the authentication module and add comprehensive test coverage",
            &empty_tools(),
        );
        assert!(d.complexity >= TaskComplexity::MultiDomain);
        assert!(d.use_orchestration);
    }

    #[test]
    fn long_horizon_full_audit() {
        let d = estimate_complexity(
            "perform a complete security audit of the project, check all dependencies for vulnerabilities, \
             run the test suite, verify git history for sensitive data, deploy to staging, \
             and generate a coverage report across all modules",
            &empty_tools(),
        );
        assert_eq!(d.complexity, TaskComplexity::LongHorizon);
        assert!(d.use_orchestration);
        assert!(d.recommended_plan_depth >= 8);
    }

    #[test]
    fn greeting_is_simple() {
        let d = estimate_complexity("hello how are you", &empty_tools());
        assert_eq!(d.complexity, TaskComplexity::SimpleExecution);
        assert!(!d.use_orchestration);
    }

    #[test]
    fn spanish_investigation() {
        let d = estimate_complexity("analizar y revisar la estructura del proyecto", &empty_tools());
        assert!(d.complexity >= TaskComplexity::StructuredTask);
    }

    #[test]
    fn spanish_multi_domain() {
        let d = estimate_complexity(
            "crear pruebas unitarias, verificar seguridad y desplegar a producción",
            &empty_tools(),
        );
        assert!(d.complexity >= TaskComplexity::MultiDomain);
        assert!(d.use_orchestration);
    }

    #[test]
    fn complexity_ordering() {
        assert!(TaskComplexity::SimpleExecution < TaskComplexity::StructuredTask);
        assert!(TaskComplexity::StructuredTask < TaskComplexity::MultiDomain);
        assert!(TaskComplexity::MultiDomain < TaskComplexity::LongHorizon);
    }

    #[test]
    fn max_rounds_increases_with_complexity() {
        let simple = estimate_complexity("read file", &empty_tools());
        let structured = estimate_complexity("find and fix the bug in auth module", &empty_tools());
        assert!(simple.recommended_max_rounds <= structured.recommended_max_rounds);
    }
}
