//! Runtime bridge: connects halcon-cli tool execution to the halcon-runtime DAG executor.
//!
//! # Architecture
//!
//! `CliToolRuntime` wraps `HalconRuntime` and registers one `LocalToolAgent` per tool
//! found in a `ToolRegistry`. Parallel tool batches are converted to a single-wave
//! `TaskDAG` (all nodes at depth 0, no dependencies) and executed via the unified
//! runtime executor.
//!
//! # Execution Model
//!
//! Parallel batch → single-wave TaskDAG where every tool node is independent.
//! The `RuntimeExecutor` launches each node concurrently via `tokio::spawn`, matching
//! the previous `buffer_unordered` approach while unifying execution under the typed
//! halcon-runtime orchestration layer.
//!
//! # Security Note
//!
//! `LocalToolAgent::invoke()` calls `tool.execute()` directly. The CLI-side security
//! gates (risk scoring, idempotency, dry-run, retry) live in `executor::execute_one_tool`
//! and are the responsibility of the caller. `CliToolRuntime` is intended for use cases
//! where those gates are already applied upstream, or for standalone delegation/testing.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use halcon_runtime::bridges::tool_agent::LocalToolAgent;
use halcon_runtime::executor::{AgentSelector, ExecutionResult, TaskDAG, TaskNode};
use halcon_runtime::runtime::{HalconRuntime, RuntimeConfig};
use halcon_tools::ToolRegistry;

use halcon_core::types::ContentBlock;

use super::super::accumulator::CompletedToolUse;
use super::super::executor::ToolExecResult;

/// A `HalconRuntime` pre-populated with `LocalToolAgent` wrappers for every tool
/// in a `ToolRegistry`.
///
/// Instantiate once per session and reuse across batches. Thread-safe because
/// `HalconRuntime`'s registry is backed by `Arc<RwLock<_>>` internally.
pub struct CliToolRuntime {
    runtime: HalconRuntime,
    /// Maps tool name → UUID of the registered `LocalToolAgent`.
    tool_to_agent: HashMap<String, Uuid>,
}

impl CliToolRuntime {
    /// Build a `CliToolRuntime` by registering one `LocalToolAgent` for each tool
    /// in `registry`. The `working_dir` is forwarded to every agent at construction time.
    pub async fn from_registry(registry: &ToolRegistry, working_dir: &str) -> Self {
        let runtime = HalconRuntime::new(RuntimeConfig::default());
        let mut tool_to_agent = HashMap::new();

        for def in registry.tool_definitions() {
            if let Some(tool) = registry.get(&def.name) {
                let agent = Arc::new(LocalToolAgent::new(tool.clone(), working_dir));
                let id = runtime.register_agent(agent).await;
                tool_to_agent.insert(def.name, id);
            }
        }

        Self {
            runtime,
            tool_to_agent,
        }
    }

    /// Number of tool agents registered in the underlying runtime.
    pub fn agent_count(&self) -> usize {
        self.tool_to_agent.len()
    }

    /// Execute a parallel batch of tool calls as a single-wave `TaskDAG`.
    ///
    /// All tools run concurrently (no inter-tool dependencies). Results are returned
    /// in deterministic order sorted by `tool_use_id`, matching the existing
    /// `execute_parallel_batch` contract.
    ///
    /// Tools not found in the registry produce error results (agent-not-found).
    pub async fn execute_parallel_batch(&self, batch: &[CompletedToolUse]) -> Vec<ToolExecResult> {
        if batch.is_empty() {
            return Vec::new();
        }

        let (dag, task_to_index) = self.build_dag(batch);

        match self.runtime.execute_dag(dag).await {
            Ok(exec_result) => self.map_results(exec_result, batch, &task_to_index),
            Err(e) => {
                tracing::error!(
                    error = %e,
                    batch_size = batch.len(),
                    "CliToolRuntime DAG execution failed — returning errors for all tools"
                );
                batch
                    .iter()
                    .map(|t| make_error_result(t, format!("Runtime execution error: {e}")))
                    .collect()
            }
        }
    }

    // ── internals ──────────────────────────────────────────────────────────────

    /// Build a single-wave `TaskDAG` from the batch.
    ///
    /// Returns the DAG and a `HashMap<task_id → batch_index>` for result mapping.
    fn build_dag(&self, batch: &[CompletedToolUse]) -> (TaskDAG, HashMap<Uuid, usize>) {
        let mut dag = TaskDAG::new();
        let mut task_to_index = HashMap::with_capacity(batch.len());

        for (idx, tool_call) in batch.iter().enumerate() {
            let task_id = Uuid::new_v4();

            let agent_selector = match self.tool_to_agent.get(&tool_call.name) {
                Some(&id) => AgentSelector::ById(id),
                None => {
                    tracing::warn!(
                        tool = %tool_call.name,
                        "No registered agent for tool — will fail at DAG execution"
                    );
                    // Fall back to ByName; the runtime will emit AgentNotFound.
                    AgentSelector::ByName(format!("tool:{}", tool_call.name))
                }
            };

            // Serialize input arguments as the agent instruction.
            let instruction = serde_json::to_string(&tool_call.input)
                .unwrap_or_else(|_| "{}".to_string());

            dag.add_node(TaskNode {
                task_id,
                instruction,
                agent_selector,
                depends_on: vec![], // all parallel — single wave
                budget: None,
                context_keys: vec![],
            });

            task_to_index.insert(task_id, idx);
        }

        (dag, task_to_index)
    }

    /// Map `ExecutionResult` from the runtime back to `Vec<ToolExecResult>`.
    fn map_results(
        &self,
        exec_result: ExecutionResult,
        batch: &[CompletedToolUse],
        task_to_index: &HashMap<Uuid, usize>,
    ) -> Vec<ToolExecResult> {
        let mut results: Vec<ToolExecResult> = exec_result
            .results
            .into_iter()
            .map(|(task_id, response)| {
                let tool_call = task_to_index
                    .get(&task_id)
                    .and_then(|&i| batch.get(i));

                let (tool_use_id, tool_name) = tool_call
                    .map(|t| (t.id.clone(), t.name.clone()))
                    .unwrap_or_default();

                match response {
                    Ok(resp) => ToolExecResult {
                        tool_use_id: tool_use_id.clone(),
                        tool_name,
                        content_block: ContentBlock::ToolResult {
                            tool_use_id,
                            content: resp.output,
                            is_error: !resp.success,
                        },
                        duration_ms: resp.usage.latency_ms,
                        was_parallel: true,
                    },
                    Err(e) => ToolExecResult {
                        tool_use_id: tool_use_id.clone(),
                        tool_name,
                        content_block: ContentBlock::ToolResult {
                            tool_use_id,
                            content: format!("Error: {e}"),
                            is_error: true,
                        },
                        duration_ms: 0,
                        was_parallel: true,
                    },
                }
            })
            .collect();

        // Deterministic ordering: match existing `execute_parallel_batch` contract.
        results.sort_by(|a, b| a.tool_use_id.cmp(&b.tool_use_id));
        results
    }
}

/// Construct an error `ToolExecResult` for a given tool call.
fn make_error_result(tool_call: &CompletedToolUse, content: String) -> ToolExecResult {
    ToolExecResult {
        tool_use_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content_block: ContentBlock::ToolResult {
            tool_use_id: tool_call.id.clone(),
            content,
            is_error: true,
        },
        duration_ms: 0,
        was_parallel: true,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use halcon_core::error::{HalconError, Result as CoreResult};
    use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};
    use halcon_core::traits::Tool;

    // ── Helpers ────────────────────────────────────────────────────────────

    struct EchoTool {
        tool_name: String,
    }

    impl EchoTool {
        fn new(name: &str) -> Self {
            Self { tool_name: name.to_string() }
        }
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str { &self.tool_name }
        fn description(&self) -> &str { "echo tool for testing" }
        fn permission_level(&self) -> PermissionLevel { PermissionLevel::ReadOnly }

        async fn execute(&self, input: ToolInput) -> CoreResult<ToolOutput> {
            Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("echo:{}", input.arguments),
                is_error: false,
                metadata: None,
            })
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
    }

    struct FailingTool {
        tool_name: String,
    }

    impl FailingTool {
        fn new(name: &str) -> Self {
            Self { tool_name: name.to_string() }
        }
    }

    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &str { &self.tool_name }
        fn description(&self) -> &str { "always fails" }
        fn permission_level(&self) -> PermissionLevel { PermissionLevel::ReadOnly }

        async fn execute(&self, _input: ToolInput) -> CoreResult<ToolOutput> {
            Err(HalconError::ToolExecutionFailed {
                tool: self.tool_name.clone(),
                message: "intentional failure".to_string(),
            })
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
    }

    fn make_registry_with(tools: Vec<Arc<dyn Tool>>) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        for tool in tools {
            reg.register(tool);
        }
        reg
    }

    fn make_echo_registry(names: &[&str]) -> ToolRegistry {
        make_registry_with(
            names
                .iter()
                .map(|&n| Arc::new(EchoTool::new(n)) as Arc<dyn Tool>)
                .collect(),
        )
    }

    fn tool_call(name: &str) -> CompletedToolUse {
        CompletedToolUse {
            id: format!("id_{name}"),
            name: name.to_string(),
            input: serde_json::json!({"key": "value"}),
        }
    }

    fn is_error(r: &ToolExecResult) -> bool {
        matches!(&r.content_block, ContentBlock::ToolResult { is_error, .. } if *is_error)
    }

    // ── Tests ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn empty_registry_has_zero_agents() {
        let reg = make_echo_registry(&[]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;
        assert_eq!(rt.agent_count(), 0);
    }

    #[tokio::test]
    async fn from_registry_registers_all_tools() {
        let reg = make_echo_registry(&["tool_a", "tool_b", "tool_c"]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;
        assert_eq!(rt.agent_count(), 3);
    }

    #[tokio::test]
    async fn empty_batch_returns_empty() {
        let reg = make_echo_registry(&["echo"]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;
        let results = rt.execute_parallel_batch(&[]).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn single_tool_executes_successfully() {
        let reg = make_echo_registry(&["echo_tool"]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;

        let batch = vec![tool_call("echo_tool")];
        let results = rt.execute_parallel_batch(&batch).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "echo_tool");
        assert_eq!(results[0].tool_use_id, "id_echo_tool");
        assert!(results[0].was_parallel);
        assert!(!is_error(&results[0]));
    }

    #[tokio::test]
    async fn parallel_batch_all_succeed() {
        let reg = make_echo_registry(&["tool_a", "tool_b", "tool_c"]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;

        let batch = vec![tool_call("tool_a"), tool_call("tool_b"), tool_call("tool_c")];
        let results = rt.execute_parallel_batch(&batch).await;

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.was_parallel));
        assert!(results.iter().all(|r| !is_error(r)));
    }

    #[tokio::test]
    async fn results_sorted_by_tool_use_id() {
        // Create tools in reverse alpha order — results must still come out sorted.
        let reg = make_echo_registry(&["zzz_tool", "aaa_tool", "mmm_tool"]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;

        let batch = vec![
            tool_call("zzz_tool"),
            tool_call("aaa_tool"),
            tool_call("mmm_tool"),
        ];
        let results = rt.execute_parallel_batch(&batch).await;

        let ids: Vec<&str> = results.iter().map(|r| r.tool_use_id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }

    #[tokio::test]
    async fn unknown_tool_produces_error_result() {
        let reg = make_echo_registry(&["known_tool"]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;

        let batch = vec![tool_call("unknown_tool")];
        let results = rt.execute_parallel_batch(&batch).await;

        assert_eq!(results.len(), 1);
        assert!(is_error(&results[0]));
        assert!(results[0].was_parallel);
    }

    #[tokio::test]
    async fn failing_tool_produces_error_result() {
        let reg = make_registry_with(vec![
            Arc::new(FailingTool::new("fail_tool")) as Arc<dyn Tool>,
        ]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;

        let batch = vec![tool_call("fail_tool")];
        let results = rt.execute_parallel_batch(&batch).await;

        assert_eq!(results.len(), 1);
        assert!(is_error(&results[0]));
        assert!(results[0].was_parallel);
        // Error message is propagated.
        match &results[0].content_block {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("Error"), "expected error message, got: {content}");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[tokio::test]
    async fn mixed_success_and_failure() {
        let reg = make_registry_with(vec![
            Arc::new(EchoTool::new("ok_tool")) as Arc<dyn Tool>,
            Arc::new(FailingTool::new("bad_tool")) as Arc<dyn Tool>,
        ]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;

        let batch = vec![tool_call("ok_tool"), tool_call("bad_tool")];
        let results = rt.execute_parallel_batch(&batch).await;

        assert_eq!(results.len(), 2);
        // Sorted by tool_use_id: "id_bad_tool" < "id_ok_tool".
        assert_eq!(results[0].tool_use_id, "id_bad_tool");
        assert_eq!(results[1].tool_use_id, "id_ok_tool");
        assert!(is_error(&results[0]));
        assert!(!is_error(&results[1]));
    }

    #[tokio::test]
    async fn parallel_batch_is_single_wave_dag() {
        let reg = make_echo_registry(&["tool_a", "tool_b", "tool_c"]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;

        let batch = vec![tool_call("tool_a"), tool_call("tool_b"), tool_call("tool_c")];
        let (dag, _) = rt.build_dag(&batch);

        let waves = dag.waves().unwrap();
        assert_eq!(waves.len(), 1, "parallel batch must produce exactly one DAG wave");
        assert_eq!(waves[0].len(), 3, "all three tools must be in the single wave");
    }

    #[tokio::test]
    async fn dag_task_to_index_covers_all_tools() {
        let reg = make_echo_registry(&["tool_a", "tool_b"]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;

        let batch = vec![tool_call("tool_a"), tool_call("tool_b")];
        let (dag, task_to_index) = rt.build_dag(&batch);

        assert_eq!(dag.nodes().len(), 2);
        assert_eq!(task_to_index.len(), 2);
        // Every task_id in the DAG maps to a valid batch index.
        for node in dag.nodes() {
            let idx = task_to_index.get(&node.task_id).copied().unwrap();
            assert!(idx < batch.len());
        }
    }

    #[tokio::test]
    async fn output_content_contains_tool_output() {
        let reg = make_echo_registry(&["echo_tool"]);
        let rt = CliToolRuntime::from_registry(&reg, "/tmp").await;

        let call = CompletedToolUse {
            id: "my_id".to_string(),
            name: "echo_tool".to_string(),
            input: serde_json::json!({"hello": "world"}),
        };
        let results = rt.execute_parallel_batch(&[call]).await;

        assert_eq!(results.len(), 1);
        match &results[0].content_block {
            ContentBlock::ToolResult { content, is_error, .. } => {
                assert!(!is_error);
                // EchoTool prepends "echo:" to the serialized args.
                assert!(content.starts_with("echo:"), "unexpected content: {content}");
            }
            _ => panic!("expected ToolResult"),
        }
    }
}
