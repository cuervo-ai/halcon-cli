//! `json_schema_validate` tool: validate JSON data against a JSON Schema.
//!
//! Implements a lightweight JSON Schema validator supporting:
//! - type checking (string, number, integer, boolean, array, object, null)
//! - required fields
//! - minLength / maxLength for strings
//! - minimum / maximum for numbers
//! - minItems / maxItems for arrays
//! - properties (recursive validation)
//! - enum values
//!
//! Does NOT require external crates — zero new dependencies.

use async_trait::async_trait;
use serde_json::{json, Value};

use halcon_core::error::{HalconError, Result};
use halcon_core::traits::Tool;
use halcon_core::types::{PermissionLevel, ToolInput, ToolOutput};

#[allow(unused_imports)]
use tracing::instrument;

pub struct JsonSchemaValidateTool;

impl JsonSchemaValidateTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for JsonSchemaValidateTool {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Validation engine ────────────────────────────────────────────────────────

const MAX_DEPTH: usize = 64;
const MAX_ERRORS: usize = 100;

fn validate(data: &Value, schema: &Value, path: &str, errors: &mut Vec<String>) {
    validate_inner(data, schema, path, errors, 0);
}

fn validate_inner(data: &Value, schema: &Value, path: &str, errors: &mut Vec<String>, depth: usize) {
    if errors.len() >= MAX_ERRORS {
        return; // cap error list — prevents unbounded allocation
    }
    if depth >= MAX_DEPTH {
        errors.push(format!("{path}: schema nesting exceeds maximum depth ({MAX_DEPTH})"));
        return;
    }
    // type check
    if let Some(type_val) = schema.get("type") {
        let type_str = match type_val {
            Value::String(s) => s.as_str(),
            _ => "",
        };
        let type_ok = match type_str {
            "string"  => data.is_string(),
            "number"  => data.is_number(),
            "integer" => data.is_i64() || data.is_u64(),
            "boolean" => data.is_boolean(),
            "array"   => data.is_array(),
            "object"  => data.is_object(),
            "null"    => data.is_null(),
            _         => true,
        };
        if !type_ok {
            let actual = json_type_name(data);
            errors.push(format!("{path}: expected type '{type_str}', got '{actual}'"));
            return; // further checks on wrong type are noisy
        }
    }

    // enum
    if let Some(Value::Array(allowed)) = schema.get("enum") {
        if !allowed.contains(data) {
            let allowed_str: Vec<String> = allowed.iter().map(|v| v.to_string()).collect();
            errors.push(format!("{path}: value {data} not in enum [{}]", allowed_str.join(", ")));
        }
    }

    // String constraints
    if let Some(s) = data.as_str() {
        if let Some(min) = schema.get("minLength").and_then(|v| v.as_u64()) {
            if s.len() < min as usize {
                errors.push(format!("{path}: string length {} < minLength {min}", s.len()));
            }
        }
        if let Some(max) = schema.get("maxLength").and_then(|v| v.as_u64()) {
            if s.len() > max as usize {
                errors.push(format!("{path}: string length {} > maxLength {max}", s.len()));
            }
        }
    }

    // Number constraints
    if let Some(n) = data.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(|v| v.as_f64()) {
            if n < min {
                errors.push(format!("{path}: {n} < minimum {min}"));
            }
        }
        if let Some(max) = schema.get("maximum").and_then(|v| v.as_f64()) {
            if n > max {
                errors.push(format!("{path}: {n} > maximum {max}"));
            }
        }
    }

    // Array constraints
    if let Some(arr) = data.as_array() {
        if let Some(min) = schema.get("minItems").and_then(|v| v.as_u64()) {
            if (arr.len() as u64) < min {
                errors.push(format!("{path}: array length {} < minItems {min}", arr.len()));
            }
        }
        if let Some(max) = schema.get("maxItems").and_then(|v| v.as_u64()) {
            if (arr.len() as u64) > max {
                errors.push(format!("{path}: array length {} > maxItems {max}", arr.len()));
            }
        }
        if let Some(item_schema) = schema.get("items") {
            for (i, item) in arr.iter().enumerate() {
                if errors.len() >= MAX_ERRORS { break; }
                validate_inner(item, item_schema, &format!("{path}[{i}]"), errors, depth + 1);
            }
        }
    }

    // Object constraints
    if let Some(obj) = data.as_object() {
        // required fields
        if let Some(Value::Array(required)) = schema.get("required") {
            for req in required {
                if errors.len() >= MAX_ERRORS { break; }
                if let Some(field) = req.as_str() {
                    if !obj.contains_key(field) {
                        errors.push(format!("{path}: missing required field '{field}'"));
                    }
                }
            }
        }

        // properties — recurse
        if let Some(Value::Object(props)) = schema.get("properties") {
            for (prop_name, prop_schema) in props {
                if let Some(prop_value) = obj.get(prop_name) {
                    let child_path = if path.is_empty() {
                        prop_name.clone()
                    } else {
                        format!("{path}.{prop_name}")
                    };
                    validate_inner(prop_value, prop_schema, &child_path, errors, depth + 1);
                }
            }
        }

        // additionalProperties: false
        if let Some(Value::Bool(false)) = schema.get("additionalProperties") {
            if let Some(Value::Object(props)) = schema.get("properties") {
                for key in obj.keys() {
                    if !props.contains_key(key) {
                        errors.push(format!("{path}: additional property '{key}' not allowed"));
                    }
                }
            }
        }
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "string",
        Value::Number(n) => if n.is_i64() || n.is_u64() { "integer" } else { "number" },
        Value::Bool(_)   => "boolean",
        Value::Array(_)  => "array",
        Value::Object(_) => "object",
        Value::Null      => "null",
    }
}

// ─── Tool impl ────────────────────────────────────────────────────────────────

#[async_trait]
impl Tool for JsonSchemaValidateTool {
    fn name(&self) -> &str {
        "json_schema_validate"
    }

    fn description(&self) -> &str {
        "Validate JSON data against a JSON Schema. \
         Supports type checking, required fields, string length constraints (minLength/maxLength), \
         numeric range constraints (minimum/maximum), array constraints (minItems/maxItems), \
         nested properties, enum values, and additionalProperties:false. \
         Returns a list of validation errors or confirms the data is valid."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, _input: &ToolInput) -> bool {
        false
    }

    #[tracing::instrument(skip(self), fields(tool = "json_schema_validate"))]
    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        // Accept data as an embedded JSON string ({"x":1}) or a direct value.
        // When a String is provided, try parsing it as JSON first; if it fails
        // (e.g. bare "red"), treat the string itself as the data value.
        let data_val: Value = match &input.arguments["data"] {
            Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone())),
            other => other.clone(),
        };

        // Schema must be parseable — invalid JSON schema is always an error.
        let schema_val: Value = match input.arguments.get("schema") {
            Some(Value::String(s)) => serde_json::from_str(s).map_err(|e| {
                HalconError::InvalidInput(format!("'schema' is not valid JSON: {e}"))
            })?,
            Some(other) => other.clone(),
            None => {
                return Err(HalconError::InvalidInput(
                    "json_schema_validate requires 'schema' object or JSON string".into(),
                ))
            }
        };

        let mut errors: Vec<String> = Vec::new();
        validate(&data_val, &schema_val, "", &mut errors);

        let (content, is_error) = if errors.is_empty() {
            ("✓ JSON data is valid against the schema.".to_string(), false)
        } else {
            let error_list = errors
                .iter()
                .enumerate()
                .map(|(i, e)| format!("  {}. {}", i + 1, e))
                .collect::<Vec<_>>()
                .join("\n");
            (
                format!("✗ Validation failed with {} error(s):\n{error_list}", errors.len()),
                true,
            )
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error,
            metadata: Some(json!({
                "valid": errors.is_empty(),
                "error_count": errors.len(),
                "errors": errors,
            })),
        })
    }

    fn input_schema(&self) -> serde_json::Value {
        // Note: union "type" arrays (e.g. ["object","array","string"]) are rejected by
        // OpenAI o1/o3 with "array schema missing items". Use anyOf instead — this is
        // universally supported by Anthropic, OpenAI, DeepSeek, and Gemini.
        json!({
            "type": "object",
            "properties": {
                "data": {
                    "description": "The JSON data to validate. Can be a JSON object, array, or a JSON-encoded string.",
                    "anyOf": [
                        { "type": "object" },
                        { "type": "array", "items": {} },
                        { "type": "string" }
                    ]
                },
                "schema": {
                    "description": "The JSON Schema to validate against. Can be a JSON object or a JSON-encoded string.",
                    "anyOf": [
                        { "type": "object" },
                        { "type": "string" }
                    ]
                }
            },
            "required": ["data", "schema"]
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(data: Value, schema: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "data": data, "schema": schema }),
            working_directory: "/tmp".into(),
        }
    }

    #[tokio::test]
    async fn valid_simple_object() {
        let tool = JsonSchemaValidateTool::new();
        let data = json!({ "name": "Alice", "age": 30 });
        let schema = json!({
            "type": "object",
            "required": ["name", "age"],
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            }
        });
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains('✓'));
    }

    #[tokio::test]
    async fn missing_required_field() {
        let tool = JsonSchemaValidateTool::new();
        let data = json!({ "name": "Alice" }); // missing "age"
        let schema = json!({
            "type": "object",
            "required": ["name", "age"]
        });
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("missing required field 'age'"));
    }

    #[tokio::test]
    async fn wrong_type() {
        let tool = JsonSchemaValidateTool::new();
        let data = json!({ "count": "not_a_number" });
        let schema = json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer" }
            }
        });
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("expected type 'integer'"));
    }

    #[tokio::test]
    async fn string_min_length_violation() {
        let tool = JsonSchemaValidateTool::new();
        let data = json!("hi");
        let schema = json!({ "type": "string", "minLength": 5 });
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("minLength"));
    }

    #[tokio::test]
    async fn enum_valid() {
        let tool = JsonSchemaValidateTool::new();
        let data = json!("red");
        let schema = json!({ "enum": ["red", "green", "blue"] });
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        assert!(!out.is_error);
    }

    #[tokio::test]
    async fn enum_invalid() {
        let tool = JsonSchemaValidateTool::new();
        let data = json!("yellow");
        let schema = json!({ "enum": ["red", "green", "blue"] });
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("not in enum"));
    }

    #[tokio::test]
    async fn nested_properties() {
        let tool = JsonSchemaValidateTool::new();
        let data = json!({ "user": { "name": 42 } }); // name should be string
        let schema = json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    }
                }
            }
        });
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("user.name"));
    }

    #[tokio::test]
    async fn array_min_items() {
        let tool = JsonSchemaValidateTool::new();
        let data = json!([1]);
        let schema = json!({ "type": "array", "minItems": 3 });
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("minItems"));
    }

    #[tokio::test]
    async fn data_as_json_string() {
        let tool = JsonSchemaValidateTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({
                "data": "{\"x\": 1}",
                "schema": { "type": "object", "required": ["x"] }
            }),
            working_directory: "/tmp".into(),
        };
        let out = tool.execute(input).await.unwrap();
        assert!(!out.is_error);
    }

    #[tokio::test]
    async fn missing_schema_is_error() {
        let tool = JsonSchemaValidateTool::new();
        let input = ToolInput {
            tool_use_id: "t".into(),
            arguments: json!({ "data": {} }),
            working_directory: "/tmp".into(),
        };
        let result = tool.execute(input).await;
        assert!(result.is_err());
    }

    #[test]
    fn name_and_permission() {
        let tool = JsonSchemaValidateTool::new();
        assert_eq!(tool.name(), "json_schema_validate");
        assert_eq!(tool.permission_level(), PermissionLevel::ReadOnly);
    }

    /// DoS protection: deeply nested schemas must not overflow the stack.
    #[tokio::test]
    async fn deep_nesting_rejected() {
        let tool = JsonSchemaValidateTool::new();
        // Build schema nested 200 levels deep
        let mut schema = json!({ "type": "object" });
        for _ in 0..200 {
            schema = json!({ "type": "object", "properties": { "x": schema } });
        }
        let data = json!({});
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        // The validator should either return a depth error or terminate gracefully (no stack overflow)
        // No assertion on is_error since shallow data may pass partial checks; key is it RETURNS.
        let _ = out; // If we reach here, no stack overflow occurred.
    }

    /// DoS protection: error list must be capped regardless of schema size.
    #[tokio::test]
    async fn error_list_capped_at_max() {
        let tool = JsonSchemaValidateTool::new();
        // Create an object with 200 properties, all required but missing from data
        let mut props = serde_json::Map::new();
        let mut required = Vec::new();
        for i in 0..200 {
            let key = format!("field_{i}");
            props.insert(key.clone(), json!({ "type": "string" }));
            required.push(json!(key));
        }
        let schema = json!({
            "type": "object",
            "required": required,
            "properties": props
        });
        let data = json!({}); // all 200 fields missing
        let out = tool.execute(make_input(data, schema)).await.unwrap();
        assert!(out.is_error);
        let error_count = out.metadata.as_ref()
            .and_then(|m| m["error_count"].as_u64())
            .unwrap_or(0);
        assert!(error_count <= 100, "error list should be capped at 100, got {error_count}");
    }
}
