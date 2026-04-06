//! JsonTransformTool — query and transform JSON using path expressions.
//!
//! Features:
//! - Extract values by dot-notation path (e.g. "user.address.city")
//! - Filter arrays by field value (e.g. "items[?name=foo]")
//! - Map: extract a field from every array element
//! - Keys: list keys of an object
//! - Pretty/compact formatting
//! - Type introspection: show the type of a value at a path

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};

pub struct JsonTransformTool;

impl JsonTransformTool {
    pub fn new() -> Self {
        Self
    }

    /// Navigate into a JSON value by a dot-separated path.
    /// Supports array index: "items.0.name" or "items[0].name"
    fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
        if path.is_empty() || path == "." {
            return Some(value);
        }
        let mut current = value;
        for segment in Self::split_path(path) {
            current = match current {
                Value::Object(map) => map.get(&segment)?,
                Value::Array(arr) => {
                    let idx: usize = segment.parse().ok()?;
                    arr.get(idx)?
                }
                _ => return None,
            };
        }
        Some(current)
    }

    /// Split path into segments, handling both "a.b.c" and "a[0].b" notation.
    fn split_path(path: &str) -> Vec<String> {
        let mut segs = vec![];
        let mut buf = String::new();
        let chars: Vec<char> = path.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '.' => {
                    if !buf.is_empty() {
                        segs.push(buf.clone());
                        buf.clear();
                    }
                }
                '[' => {
                    if !buf.is_empty() {
                        segs.push(buf.clone());
                        buf.clear();
                    }
                    i += 1;
                    while i < chars.len() && chars[i] != ']' {
                        buf.push(chars[i]);
                        i += 1;
                    }
                    segs.push(buf.clone());
                    buf.clear();
                }
                ']' => {}
                c => buf.push(c),
            }
            i += 1;
        }
        if !buf.is_empty() {
            segs.push(buf);
        }
        segs
    }

    /// Filter array elements where field == value.
    /// Syntax: "items[?status=active]" → finds key=items, filters where .status == "active"
    fn filter_array(array: &Value, field: &str, expected: &str) -> Value {
        match array {
            Value::Array(arr) => {
                let filtered: Vec<Value> = arr
                    .iter()
                    .filter(|item| {
                        if let Some(v) = item.get(field) {
                            let s = match v {
                                Value::String(s) => s.clone(),
                                Value::Number(n) => n.to_string(),
                                Value::Bool(b) => b.to_string(),
                                _ => return false,
                            };
                            s == expected
                        } else {
                            false
                        }
                    })
                    .cloned()
                    .collect();
                Value::Array(filtered)
            }
            _ => Value::Array(vec![]),
        }
    }

    /// Map: extract a single field from every element of an array.
    fn map_field(array: &Value, field: &str) -> Value {
        match array {
            Value::Array(arr) => {
                let mapped: Vec<Value> = arr
                    .iter()
                    .filter_map(|item| item.get(field).cloned())
                    .collect();
                Value::Array(mapped)
            }
            _ => Value::Array(vec![]),
        }
    }

    /// Return type name of a JSON value.
    fn type_of(v: &Value) -> &'static str {
        match v {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) => {
                if n.is_f64() {
                    "float"
                } else {
                    "integer"
                }
            }
            Value::String(_) => "string",
            Value::Array(a) => {
                if a.is_empty() {
                    "array (empty)"
                } else {
                    "array"
                }
            }
            Value::Object(o) => {
                if o.is_empty() {
                    "object (empty)"
                } else {
                    "object"
                }
            }
        }
    }
}

impl Default for JsonTransformTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for JsonTransformTool {
    fn name(&self) -> &str {
        "json_transform"
    }

    fn description(&self) -> &str {
        "Query and transform JSON data. Supports path extraction (dot notation), \
         array filtering by field value, field mapping over arrays, key listing, \
         type introspection, and pretty/compact formatting. Useful for working with \
         API responses, config files, and structured data."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "json": {
                    "type": "string",
                    "description": "JSON string to transform."
                },
                "operation": {
                    "type": "string",
                    "enum": ["get", "filter", "map", "keys", "type", "pretty", "compact", "length"],
                    "description": "Operation: 'get' (extract path), 'filter' (filter array), 'map' (extract field from all array items), 'keys' (list object keys), 'type' (show type at path), 'pretty'/'compact' (reformat), 'length' (count items)."
                },
                "path": {
                    "type": "string",
                    "description": "Dot-notation path for get/type/filter/map. E.g. 'user.address.city' or 'items[0].name'."
                },
                "filter_field": {
                    "type": "string",
                    "description": "For filter operation: the field to filter array elements by."
                },
                "filter_value": {
                    "type": "string",
                    "description": "For filter operation: the expected value of filter_field."
                },
                "map_field": {
                    "type": "string",
                    "description": "For map operation: the field to extract from each array element."
                },
                "output": {
                    "type": "string",
                    "enum": ["text", "json"],
                    "description": "Output format (default: text with pretty-printed result)."
                }
            },
            "required": ["json", "operation"]
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;

        let json_str = match args["json"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "Missing required 'json'.".into(),
                    is_error: true,
                    metadata: None,
                })
            }
        };

        let operation = args["operation"].as_str().unwrap_or("pretty");

        let parsed: Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("JSON parse error: {e}"),
                    is_error: true,
                    metadata: None,
                })
            }
        };

        let output_fmt = args["output"].as_str().unwrap_or("text");

        let result: Value = match operation {
            "pretty" => parsed,
            "compact" => {
                let compact = serde_json::to_string(&parsed).unwrap_or_default();
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: compact,
                    is_error: false,
                    metadata: None,
                });
            }
            "get" => {
                let path = args["path"].as_str().unwrap_or(".");
                match Self::get_path(&parsed, path) {
                    Some(v) => v.clone(),
                    None => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("Path '{}' not found", path),
                            is_error: true,
                            metadata: None,
                        })
                    }
                }
            }
            "type" => {
                let path = args["path"].as_str().unwrap_or(".");
                match Self::get_path(&parsed, path) {
                    Some(v) => {
                        let t = Self::type_of(v);
                        let out = if output_fmt == "json" {
                            json!({ "path": path, "type": t }).to_string()
                        } else {
                            format!("Type at '{}': {}", path, t)
                        };
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: out,
                            is_error: false,
                            metadata: Some(json!({ "type": t })),
                        });
                    }
                    None => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("Path '{}' not found", path),
                            is_error: true,
                            metadata: None,
                        })
                    }
                }
            }
            "keys" => {
                let path = args["path"].as_str().unwrap_or(".");
                let target = match Self::get_path(&parsed, path) {
                    Some(v) => v,
                    None => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("Path '{}' not found", path),
                            is_error: true,
                            metadata: None,
                        })
                    }
                };
                match target {
                    Value::Object(map) => {
                        let keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
                        let out = if output_fmt == "json" {
                            serde_json::to_string_pretty(
                                &json!({ "keys": keys, "count": keys.len() }),
                            )
                            .unwrap_or_default()
                        } else {
                            format!(
                                "Keys at '{}' ({}):\n{}",
                                path,
                                keys.len(),
                                keys.iter()
                                    .map(|k| format!("  - {k}"))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            )
                        };
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: out,
                            is_error: false,
                            metadata: Some(json!({ "count": keys.len() })),
                        });
                    }
                    _ => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!(
                                "Value at '{}' is not an object (type: {})",
                                path,
                                Self::type_of(target)
                            ),
                            is_error: true,
                            metadata: None,
                        })
                    }
                }
            }
            "length" => {
                let path = args["path"].as_str().unwrap_or(".");
                let target = match Self::get_path(&parsed, path) {
                    Some(v) => v,
                    None => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("Path '{}' not found", path),
                            is_error: true,
                            metadata: None,
                        })
                    }
                };
                let len = match target {
                    Value::Array(a) => a.len(),
                    Value::Object(o) => o.len(),
                    Value::String(s) => s.len(),
                    _ => 0,
                };
                let out = if output_fmt == "json" {
                    json!({ "path": path, "length": len }).to_string()
                } else {
                    format!("Length at '{}': {}", path, len)
                };
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: out,
                    is_error: false,
                    metadata: Some(json!({ "length": len })),
                });
            }
            "filter" => {
                let path = args["path"].as_str().unwrap_or(".");
                let field = args["filter_field"].as_str().unwrap_or("");
                let value = args["filter_value"].as_str().unwrap_or("");
                let target = match Self::get_path(&parsed, path) {
                    Some(v) => v,
                    None => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("Path '{}' not found", path),
                            is_error: true,
                            metadata: None,
                        })
                    }
                };
                Self::filter_array(target, field, value)
            }
            "map" => {
                let path = args["path"].as_str().unwrap_or(".");
                let field = args["map_field"].as_str().unwrap_or("");
                let target = match Self::get_path(&parsed, path) {
                    Some(v) => v,
                    None => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: format!("Path '{}' not found", path),
                            is_error: true,
                            metadata: None,
                        })
                    }
                };
                Self::map_field(target, field)
            }
            _ => parsed,
        };

        let content = serde_json::to_string_pretty(&result).unwrap_or_default();

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(
                json!({ "operation": operation, "result_type": Self::type_of(&result) }),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halcon_core::types::ToolInput;

    fn ti(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    const SAMPLE: &str =
        r#"{"name":"Alice","age":30,"address":{"city":"NYC","zip":"10001"},"tags":["rust","dev"]}"#;

    #[test]
    fn tool_metadata() {
        let t = JsonTransformTool::new();
        assert_eq!(t.name(), "json_transform");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let s = t.input_schema();
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"], json!(["json", "operation"]));
    }

    #[tokio::test]
    async fn get_top_level_field() {
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(
                json!({ "json": SAMPLE, "operation": "get", "path": "name" }),
            ))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("Alice"));
    }

    #[tokio::test]
    async fn get_nested_field() {
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(
                json!({ "json": SAMPLE, "operation": "get", "path": "address.city" }),
            ))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("NYC"));
    }

    #[tokio::test]
    async fn get_array_index() {
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(
                json!({ "json": SAMPLE, "operation": "get", "path": "tags[0]" }),
            ))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("rust"));
    }

    #[tokio::test]
    async fn keys_operation() {
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(
                json!({ "json": SAMPLE, "operation": "keys", "path": "." }),
            ))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("name") || out.content.contains("age"));
    }

    #[tokio::test]
    async fn type_operation() {
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(
                json!({ "json": SAMPLE, "operation": "type", "path": "age" }),
            ))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("integer")
                || out.metadata.as_ref().and_then(|m| m["type"].as_str()) == Some("integer")
        );
    }

    #[tokio::test]
    async fn filter_array() {
        let data = r#"{"users":[{"name":"Alice","active":true},{"name":"Bob","active":false},{"name":"Carol","active":true}]}"#;
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(json!({
                "json": data,
                "operation": "filter",
                "path": "users",
                "filter_field": "name",
                "filter_value": "Alice"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("Alice"));
        assert!(!out.content.contains("Bob"));
    }

    #[tokio::test]
    async fn map_operation() {
        let data = r#"{"items":[{"id":1,"name":"foo"},{"id":2,"name":"bar"}]}"#;
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(json!({
                "json": data,
                "operation": "map",
                "path": "items",
                "map_field": "name"
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("foo"));
        assert!(out.content.contains("bar"));
    }

    #[tokio::test]
    async fn length_operation() {
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(
                json!({ "json": SAMPLE, "operation": "length", "path": "tags" }),
            ))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.metadata.as_ref().and_then(|m| m["length"].as_u64()),
            Some(2)
        );
    }

    #[tokio::test]
    async fn invalid_json_error() {
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(json!({ "json": "{broken", "operation": "pretty" })))
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn missing_path_error() {
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(
                json!({ "json": SAMPLE, "operation": "get", "path": "nonexistent.field" }),
            ))
            .await
            .unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn pretty_format() {
        let compact = r#"{"a":1,"b":2}"#;
        let t = JsonTransformTool::new();
        let out = t
            .execute(ti(json!({ "json": compact, "operation": "pretty" })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains('\n'));
    }
}
