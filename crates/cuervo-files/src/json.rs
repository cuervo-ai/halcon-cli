//! JSON handler: structured extraction with schema detection.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::detect::{FileInfo, FileType};
use crate::handler::{estimate_tokens_from_text, truncate_to_budget, FileContent, FileHandler};
use crate::Error;

/// Handler for JSON files with schema detection and smart truncation.
pub struct JsonHandler;

#[async_trait]
impl FileHandler for JsonHandler {
    fn name(&self) -> &str {
        "json"
    }

    fn supported_types(&self) -> &[FileType] {
        &[FileType::Json]
    }

    fn estimate_tokens(&self, info: &FileInfo) -> usize {
        // JSON is token-heavy (~3 chars per token due to braces, quotes, colons).
        (info.size_bytes as usize).div_ceil(3)
    }

    async fn extract(&self, info: &FileInfo, token_budget: usize) -> Result<FileContent, Error> {
        let raw = tokio::fs::read_to_string(&info.path)
            .await
            .map_err(|e| Error::Io {
                path: info.path.clone(),
                source: e,
            })?;

        // Parse to detect structure.
        let parsed: Result<Value, _> = serde_json::from_str(&raw);

        match parsed {
            Ok(value) => {
                let schema = describe_schema(&value);
                let pretty = serde_json::to_string_pretty(&value).unwrap_or(raw);
                let (text, truncated) = truncate_to_budget(&pretty, token_budget);
                let estimated_tokens = estimate_tokens_from_text(&text);

                Ok(FileContent {
                    text,
                    estimated_tokens,
                    metadata: json!({
                        "format": "json",
                        "valid": true,
                        "schema": schema,
                        "size_bytes": info.size_bytes,
                    }),
                    truncated,
                })
            }
            Err(e) => {
                // Invalid JSON: return raw text with error metadata.
                let (text, truncated) = truncate_to_budget(&raw, token_budget);
                let estimated_tokens = estimate_tokens_from_text(&text);

                Ok(FileContent {
                    text,
                    estimated_tokens,
                    metadata: json!({
                        "format": "json",
                        "valid": false,
                        "parse_error": e.to_string(),
                        "size_bytes": info.size_bytes,
                    }),
                    truncated,
                })
            }
        }
    }
}

/// Describe the schema of a JSON value (type, keys, array element count).
fn describe_schema(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            let key_types: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), json!(value_type(v))))
                .collect();
            json!({
                "type": "object",
                "keys": keys,
                "key_types": key_types,
                "key_count": map.len(),
            })
        }
        Value::Array(arr) => {
            let element_type = if arr.is_empty() {
                "unknown".to_string()
            } else {
                value_type(&arr[0])
            };
            json!({
                "type": "array",
                "length": arr.len(),
                "element_type": element_type,
            })
        }
        _ => json!({ "type": value_type(value) }),
    }
}

/// Get the JSON type name for a value.
fn value_type(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "boolean".into(),
        Value::Number(_) => "number".into(),
        Value::String(_) => "string".into(),
        Value::Array(_) => "array".into(),
        Value::Object(_) => "object".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_info(path: &std::path::Path, size: u64) -> FileInfo {
        FileInfo {
            path: path.to_path_buf(),
            file_type: FileType::Json,
            mime_type: None,
            size_bytes: size,
            is_binary: false,
        }
    }

    #[test]
    fn estimate_tokens_json() {
        let info = FileInfo {
            path: PathBuf::from("test.json"),
            file_type: FileType::Json,
            mime_type: None,
            size_bytes: 300,
            is_binary: false,
        };
        assert_eq!(JsonHandler.estimate_tokens(&info), 100);
    }

    #[tokio::test]
    async fn extract_valid_json_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("data.json");
        let content = r#"{"name": "cuervo", "version": "0.1.0", "features": ["a", "b"]}"#;
        tokio::fs::write(&path, content).await.unwrap();

        let info = make_info(&path, content.len() as u64);
        let result = JsonHandler.extract(&info, 1000).await.unwrap();

        assert!(result.metadata["valid"].as_bool().unwrap());
        assert_eq!(result.metadata["schema"]["type"], "object");
        assert_eq!(result.metadata["schema"]["key_count"], 3);
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn extract_valid_json_array() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("arr.json");
        let content = r#"[1, 2, 3, 4, 5]"#;
        tokio::fs::write(&path, content).await.unwrap();

        let info = make_info(&path, content.len() as u64);
        let result = JsonHandler.extract(&info, 1000).await.unwrap();

        assert!(result.metadata["valid"].as_bool().unwrap());
        assert_eq!(result.metadata["schema"]["type"], "array");
        assert_eq!(result.metadata["schema"]["length"], 5);
        assert_eq!(result.metadata["schema"]["element_type"], "number");
    }

    #[tokio::test]
    async fn extract_invalid_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        let content = "not valid json {{{";
        tokio::fs::write(&path, content).await.unwrap();

        let info = make_info(&path, content.len() as u64);
        let result = JsonHandler.extract(&info, 1000).await.unwrap();

        assert!(!result.metadata["valid"].as_bool().unwrap());
        assert!(result.metadata["parse_error"].as_str().is_some());
    }

    #[tokio::test]
    async fn extract_truncates_large_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("big.json");
        let arr: Vec<u32> = (0..10000).collect();
        let content = serde_json::to_string(&arr).unwrap();
        tokio::fs::write(&path, &content).await.unwrap();

        let info = make_info(&path, content.len() as u64);
        let result = JsonHandler.extract(&info, 50).await.unwrap();

        assert!(result.truncated);
        assert!(result.text.len() < content.len());
    }

    #[test]
    fn describe_schema_object() {
        let v: Value = json!({"a": 1, "b": "hello", "c": [1,2,3]});
        let schema = describe_schema(&v);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["key_count"], 3);
    }

    #[test]
    fn describe_schema_array() {
        let v: Value = json!([{"x": 1}, {"x": 2}]);
        let schema = describe_schema(&v);
        assert_eq!(schema["type"], "array");
        assert_eq!(schema["length"], 2);
        assert_eq!(schema["element_type"], "object");
    }

    #[test]
    fn describe_schema_primitive() {
        let v: Value = json!(42);
        let schema = describe_schema(&v);
        assert_eq!(schema["type"], "number");
    }

    #[test]
    fn value_type_all() {
        assert_eq!(value_type(&json!(null)), "null");
        assert_eq!(value_type(&json!(true)), "boolean");
        assert_eq!(value_type(&json!(42)), "number");
        assert_eq!(value_type(&json!("hello")), "string");
        assert_eq!(value_type(&json!([])), "array");
        assert_eq!(value_type(&json!({})), "object");
    }

    #[test]
    fn handler_name() {
        assert_eq!(JsonHandler.name(), "json");
    }

    #[test]
    fn supported_types_json_only() {
        assert_eq!(JsonHandler.supported_types(), &[FileType::Json]);
    }

    #[tokio::test]
    async fn extract_empty_json_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("empty.json");
        tokio::fs::write(&path, "{}").await.unwrap();

        let info = make_info(&path, 2);
        let result = JsonHandler.extract(&info, 1000).await.unwrap();

        assert!(result.metadata["valid"].as_bool().unwrap());
        assert_eq!(result.metadata["schema"]["type"], "object");
        assert_eq!(result.metadata["schema"]["key_count"], 0);
    }

    #[tokio::test]
    async fn extract_empty_json_array() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("empty_arr.json");
        tokio::fs::write(&path, "[]").await.unwrap();

        let info = make_info(&path, 2);
        let result = JsonHandler.extract(&info, 1000).await.unwrap();

        assert!(result.metadata["valid"].as_bool().unwrap());
        assert_eq!(result.metadata["schema"]["type"], "array");
        assert_eq!(result.metadata["schema"]["length"], 0);
    }

    #[tokio::test]
    async fn extract_json_null_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("null.json");
        tokio::fs::write(&path, "null").await.unwrap();

        let info = make_info(&path, 4);
        let result = JsonHandler.extract(&info, 1000).await.unwrap();

        assert!(result.metadata["valid"].as_bool().unwrap());
        assert_eq!(result.metadata["schema"]["type"], "null");
    }

    #[tokio::test]
    async fn extract_deeply_nested_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("deep.json");
        // Build 100 levels of nesting
        let mut content = String::new();
        for _ in 0..100 {
            content.push_str("{\"n\":");
        }
        content.push_str("1");
        for _ in 0..100 {
            content.push('}');
        }
        tokio::fs::write(&path, &content).await.unwrap();

        let info = make_info(&path, content.len() as u64);
        let result = JsonHandler.extract(&info, 10_000).await.unwrap();

        assert!(result.metadata["valid"].as_bool().unwrap());
        assert_eq!(result.metadata["schema"]["type"], "object");
    }

    #[tokio::test]
    async fn extract_json_zero_budget() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("data.json");
        tokio::fs::write(&path, r#"{"key": "value"}"#).await.unwrap();

        let info = make_info(&path, 16);
        let result = JsonHandler.extract(&info, 0).await.unwrap();

        assert!(result.truncated);
    }

    #[tokio::test]
    async fn extract_json_with_unicode() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("unicode.json");
        let content = r#"{"emoji": "🦀", "japanese": "日本語", "key": "value"}"#;
        tokio::fs::write(&path, content).await.unwrap();

        let info = make_info(&path, content.len() as u64);
        let result = JsonHandler.extract(&info, 1000).await.unwrap();

        assert!(result.metadata["valid"].as_bool().unwrap());
        assert!(result.text.contains("🦀"));
    }

    #[test]
    fn describe_schema_empty_array() {
        let v: Value = json!([]);
        let schema = describe_schema(&v);
        assert_eq!(schema["type"], "array");
        assert_eq!(schema["length"], 0);
        assert_eq!(schema["element_type"], "unknown");
    }
}
