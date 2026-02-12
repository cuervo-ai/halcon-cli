//! Local tool bridge: wraps a single cuervo-core Tool as a RuntimeAgent.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::agent::{
    AgentCapability, AgentDescriptor, AgentHealth, AgentKind, AgentRequest, AgentResponse,
    AgentUsage, Artifact, ArtifactKind, ProtocolSupport, RuntimeAgent,
};
use crate::error::{Result, RuntimeError};

use cuervo_core::traits::Tool;
use cuervo_core::types::ToolInput;

/// Wraps a single `Tool` as a RuntimeAgent.
///
/// Maps AgentRequest.instruction to tool arguments (JSON),
/// executes the tool, and maps the output to AgentResponse.
pub struct LocalToolAgent {
    descriptor: AgentDescriptor,
    tool: Arc<dyn Tool>,
    working_dir: String,
}

impl LocalToolAgent {
    pub fn new(tool: Arc<dyn Tool>, working_dir: &str) -> Self {
        let tool_name = tool.name().to_string();
        let capability = match tool_name.as_str() {
            "file_read" | "file_write" | "file_edit" => AgentCapability::FileOperations,
            "bash" => AgentCapability::ShellExecution,
            "grep" | "glob" => AgentCapability::Research,
            _ => AgentCapability::Custom(tool_name.clone()),
        };

        Self {
            descriptor: AgentDescriptor {
                id: Uuid::new_v4(),
                name: format!("tool:{tool_name}"),
                agent_kind: AgentKind::Plugin,
                capabilities: vec![capability],
                protocols: vec![ProtocolSupport::Native],
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("tool_name".to_string(), serde_json::json!(tool_name));
                    m.insert(
                        "permission_level".to_string(),
                        serde_json::json!(format!("{:?}", tool.permission_level())),
                    );
                    m
                },
                max_concurrency: 5,
            },
            tool,
            working_dir: working_dir.to_string(),
        }
    }
}

#[async_trait]
impl RuntimeAgent for LocalToolAgent {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, request: AgentRequest) -> Result<AgentResponse> {
        let start = std::time::Instant::now();

        // Parse instruction as JSON tool arguments, or use it as a simple text arg
        let args: serde_json::Value = serde_json::from_str(&request.instruction)
            .unwrap_or_else(|_| serde_json::json!({"input": request.instruction}));

        let tool_input = ToolInput {
            tool_use_id: request.request_id.to_string(),
            arguments: args,
            working_directory: self.working_dir.clone(),
        };

        let result = self.tool.execute(tool_input).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                let artifacts = if output.content.len() > 1000 {
                    vec![Artifact {
                        kind: ArtifactKind::Log,
                        path: None,
                        content: output.content.clone(),
                    }]
                } else {
                    vec![]
                };

                let success = !output.is_error;
                let text = if output.is_error {
                    format!("Error: {}", output.content)
                } else {
                    output.content
                };

                Ok(AgentResponse {
                    request_id: request.request_id,
                    success,
                    output: text,
                    artifacts,
                    usage: AgentUsage {
                        latency_ms,
                        rounds: 1,
                        ..Default::default()
                    },
                    metadata: HashMap::new(),
                })
            }
            Err(e) => Err(RuntimeError::Execution(format!(
                "tool '{}' failed: {e}",
                self.tool.name()
            ))),
        }
    }

    async fn health(&self) -> AgentHealth {
        AgentHealth::Healthy // Local tools are always available
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuervo_core::error::Result as CoreResult;
    use cuervo_core::types::PermissionLevel;

    struct MockTool {
        name: String,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "mock tool"
        }

        fn permission_level(&self) -> PermissionLevel {
            PermissionLevel::ReadOnly
        }

        async fn execute(&self, input: ToolInput) -> CoreResult<cuervo_core::types::ToolOutput> {
            Ok(cuervo_core::types::ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("mock executed with: {}", input.arguments),
                is_error: false,
                metadata: None,
            })
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
    }

    struct FailingTool;

    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &str {
            "fail_tool"
        }

        fn description(&self) -> &str {
            "always fails"
        }

        fn permission_level(&self) -> PermissionLevel {
            PermissionLevel::ReadOnly
        }

        async fn execute(&self, _input: ToolInput) -> CoreResult<cuervo_core::types::ToolOutput> {
            Err(cuervo_core::error::CuervoError::ToolExecutionFailed {
                tool: "fail_tool".to_string(),
                message: "always fails".to_string(),
            })
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
    }

    #[test]
    fn descriptor_file_read() {
        let tool: Arc<dyn Tool> = Arc::new(MockTool {
            name: "file_read".to_string(),
        });
        let agent = LocalToolAgent::new(tool, "/tmp");
        assert_eq!(agent.descriptor().name, "tool:file_read");
        assert_eq!(agent.descriptor().capabilities[0], AgentCapability::FileOperations);
    }

    #[test]
    fn descriptor_bash() {
        let tool: Arc<dyn Tool> = Arc::new(MockTool {
            name: "bash".to_string(),
        });
        let agent = LocalToolAgent::new(tool, "/tmp");
        assert_eq!(agent.descriptor().capabilities[0], AgentCapability::ShellExecution);
    }

    #[test]
    fn descriptor_grep() {
        let tool: Arc<dyn Tool> = Arc::new(MockTool {
            name: "grep".to_string(),
        });
        let agent = LocalToolAgent::new(tool, "/tmp");
        assert_eq!(agent.descriptor().capabilities[0], AgentCapability::Research);
    }

    #[test]
    fn descriptor_custom_tool() {
        let tool: Arc<dyn Tool> = Arc::new(MockTool {
            name: "my_special_tool".to_string(),
        });
        let agent = LocalToolAgent::new(tool, "/tmp");
        assert_eq!(
            agent.descriptor().capabilities[0],
            AgentCapability::Custom("my_special_tool".to_string())
        );
    }

    #[tokio::test]
    async fn invoke_with_json_args() {
        let tool: Arc<dyn Tool> = Arc::new(MockTool {
            name: "test".to_string(),
        });
        let agent = LocalToolAgent::new(tool, "/tmp");
        let req = AgentRequest::new(r#"{"path": "test.rs"}"#);
        let resp = agent.invoke(req).await.unwrap();
        assert!(resp.success);
        assert!(resp.output.contains("mock executed"));
    }

    #[tokio::test]
    async fn invoke_with_plain_text() {
        let tool: Arc<dyn Tool> = Arc::new(MockTool {
            name: "test".to_string(),
        });
        let agent = LocalToolAgent::new(tool, "/tmp");
        let req = AgentRequest::new("plain text instruction");
        let resp = agent.invoke(req).await.unwrap();
        assert!(resp.success);
    }

    #[tokio::test]
    async fn invoke_failing_tool() {
        let tool: Arc<dyn Tool> = Arc::new(FailingTool);
        let agent = LocalToolAgent::new(tool, "/tmp");
        let req = AgentRequest::new("anything");
        let result = agent.invoke(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn health_always_healthy() {
        let tool: Arc<dyn Tool> = Arc::new(MockTool {
            name: "test".to_string(),
        });
        let agent = LocalToolAgent::new(tool, "/tmp");
        assert_eq!(agent.health().await, AgentHealth::Healthy);
    }

    #[tokio::test]
    async fn shutdown_ok() {
        let tool: Arc<dyn Tool> = Arc::new(MockTool {
            name: "test".to_string(),
        });
        let agent = LocalToolAgent::new(tool, "/tmp");
        assert!(agent.shutdown().await.is_ok());
    }
}
