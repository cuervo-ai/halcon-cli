use std::collections::HashMap;
use std::sync::Arc;

use halcon_core::traits::Tool;
use halcon_core::types::ToolDefinition;

/// Registry of available tools.
///
/// Tools register themselves and can be looked up by name.
/// The registry also generates tool definitions for model API calls.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Generate tool definitions for the model API.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }

    /// Validate a tool's input against its declared JSON schema.
    ///
    /// Performs lightweight structural validation:
    /// - Checks that the input is a JSON object (tools expect named parameters)
    /// - Validates required fields are present
    /// - Checks top-level field types match the schema (string, number, boolean, array, object)
    ///
    /// Returns `Ok(())` if valid, or an error describing the first violation found.
    /// This is intentionally lenient — it catches the most common LLM output errors
    /// (missing required fields, wrong root type) without a full JSON Schema validator.
    pub fn validate_tool_input(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Result<(), ToolInputError> {
        let tool = self
            .tools
            .get(tool_name)
            .ok_or_else(|| ToolInputError::UnknownTool(tool_name.to_string()))?;

        let schema = tool.input_schema();

        // Tool inputs must be JSON objects (named parameters).
        let input_obj = input.as_object().ok_or(ToolInputError::NotAnObject {
            tool: tool_name.to_string(),
            actual_type: json_type_name(input).to_string(),
        })?;

        // Check required fields.
        if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
            for req in required {
                if let Some(field_name) = req.as_str() {
                    if !input_obj.contains_key(field_name) {
                        return Err(ToolInputError::MissingRequired {
                            tool: tool_name.to_string(),
                            field: field_name.to_string(),
                        });
                    }
                }
            }
        }

        // Check top-level field types against schema properties.
        if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
            for (key, value) in input_obj {
                if let Some(prop_schema) = properties.get(key) {
                    if let Some(expected_type) = prop_schema.get("type").and_then(|t| t.as_str()) {
                        if !json_type_matches(value, expected_type) {
                            return Err(ToolInputError::TypeMismatch {
                                tool: tool_name.to_string(),
                                field: key.clone(),
                                expected: expected_type.to_string(),
                                actual: json_type_name(value).to_string(),
                            });
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Error returned when tool input validation fails.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolInputError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Tool '{tool}' expects a JSON object, got {actual_type}")]
    NotAnObject { tool: String, actual_type: String },

    #[error("Tool '{tool}' requires field '{field}' which is missing")]
    MissingRequired { tool: String, field: String },

    #[error("Tool '{tool}' field '{field}': expected {expected}, got {actual}")]
    TypeMismatch {
        tool: String,
        field: String,
        expected: String,
        actual: String,
    },
}

/// Returns the JSON type name for a value.
fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Checks if a JSON value matches the expected JSON Schema type string.
fn json_type_matches(value: &serde_json::Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" | "integer" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true, // Unknown type — don't reject
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::PermissionLevel;

    // Helper macro to define a mock tool with minimal boilerplate.
    macro_rules! mock_tool {
        ($name:expr, $schema:expr) => {{
            struct MockTool;
            #[async_trait::async_trait]
            impl halcon_core::traits::Tool for MockTool {
                fn name(&self) -> &str {
                    $name
                }
                fn description(&self) -> &str {
                    "mock"
                }
                fn permission_level(&self) -> PermissionLevel {
                    PermissionLevel::ReadOnly
                }
                async fn execute_inner(
                    &self,
                    _input: halcon_core::types::ToolInput,
                ) -> halcon_core::error::Result<halcon_core::types::ToolOutput> {
                    unimplemented!()
                }
                fn input_schema(&self) -> serde_json::Value {
                    $schema
                }
            }
            Arc::new(MockTool) as Arc<dyn halcon_core::traits::Tool>
        }};
    }

    #[test]
    fn validate_missing_required_field() {
        let mut registry = ToolRegistry::new();
        registry.register(mock_tool!(
            "file_read",
            serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" }
                }
            })
        ));

        // Missing required field.
        let result = registry.validate_tool_input("file_read", &serde_json::json!({}));
        assert!(matches!(
            result,
            Err(ToolInputError::MissingRequired { .. })
        ));

        // Valid input.
        let result = registry
            .validate_tool_input("file_read", &serde_json::json!({"path": "/tmp/test.txt"}));
        assert!(result.is_ok());

        // Wrong type.
        let result = registry.validate_tool_input("file_read", &serde_json::json!({"path": 42}));
        assert!(matches!(result, Err(ToolInputError::TypeMismatch { .. })));
    }

    #[test]
    fn validate_not_an_object() {
        let mut registry = ToolRegistry::new();
        registry.register(mock_tool!("test", serde_json::json!({"type": "object"})));

        let result = registry.validate_tool_input("test", &serde_json::json!("not an object"));
        assert!(matches!(result, Err(ToolInputError::NotAnObject { .. })));
    }

    #[test]
    fn validate_unknown_tool() {
        let registry = ToolRegistry::new();
        let result = registry.validate_tool_input("nonexistent", &serde_json::json!({}));
        assert!(matches!(result, Err(ToolInputError::UnknownTool(_))));
    }
}
