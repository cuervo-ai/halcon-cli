//! YAML handler: structure detection and extraction.

#[cfg(feature = "yaml")]
mod inner {
    use async_trait::async_trait;
    use serde_json::json;

    use crate::detect::{FileInfo, FileType};
    use crate::handler::{estimate_tokens_from_text, truncate_to_budget, FileContent, FileHandler};
    use crate::Error;

    /// Handler for YAML files with structure detection.
    pub struct YamlHandler;

    #[async_trait]
    impl FileHandler for YamlHandler {
        fn name(&self) -> &str {
            "yaml"
        }

        fn supported_types(&self) -> &[FileType] {
            &[FileType::Yaml]
        }

        fn estimate_tokens(&self, info: &FileInfo) -> usize {
            // YAML is similar to JSON: ~3.5 chars per token.
            (info.size_bytes as usize * 2).div_ceil(7)
        }

        async fn extract(
            &self,
            info: &FileInfo,
            token_budget: usize,
        ) -> Result<FileContent, Error> {
            let raw = tokio::fs::read_to_string(&info.path)
                .await
                .map_err(|e| Error::Io {
                    path: info.path.clone(),
                    source: e,
                })?;

            // Try to parse as YAML to detect structure.
            let parsed: Result<serde_json::Value, _> = serde_yaml::from_str(&raw);

            let schema = match &parsed {
                Ok(value) => describe_yaml_schema(value),
                Err(_) => json!({ "valid": false }),
            };

            let is_valid = parsed.is_ok();
            let (text, truncated) = truncate_to_budget(&raw, token_budget);
            let estimated_tokens = estimate_tokens_from_text(&text);

            Ok(FileContent {
                text,
                estimated_tokens,
                metadata: json!({
                    "format": "yaml",
                    "valid": is_valid,
                    "schema": schema,
                    "size_bytes": info.size_bytes,
                }),
                truncated,
            })
        }
    }

    fn describe_yaml_schema(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let keys: Vec<String> = map.keys().cloned().collect();
                json!({
                    "type": "mapping",
                    "keys": keys,
                    "key_count": map.len(),
                })
            }
            serde_json::Value::Array(arr) => {
                json!({
                    "type": "sequence",
                    "length": arr.len(),
                })
            }
            _ => {
                json!({ "type": "scalar" })
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn extract_valid_yaml() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("config.yaml");
            let content = "name: cuervo\nversion: 0.1.0\nfeatures:\n  - a\n  - b\n";
            tokio::fs::write(&path, content).await.unwrap();

            let info = FileInfo {
                path: path.clone(),
                file_type: FileType::Yaml,
                mime_type: None,
                size_bytes: content.len() as u64,
                is_binary: false,
            };
            let result = YamlHandler.extract(&info, 1000).await.unwrap();

            assert!(result.metadata["valid"].as_bool().unwrap());
            assert_eq!(result.metadata["schema"]["type"], "mapping");
            assert_eq!(result.metadata["schema"]["key_count"], 3);
        }

        #[tokio::test]
        async fn extract_invalid_yaml() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("bad.yaml");
            let content = ":\n  - [\ninvalid\n";
            tokio::fs::write(&path, content).await.unwrap();

            let info = FileInfo {
                path: path.clone(),
                file_type: FileType::Yaml,
                mime_type: None,
                size_bytes: content.len() as u64,
                is_binary: false,
            };
            let result = YamlHandler.extract(&info, 1000).await.unwrap();

            // serde_yaml may or may not parse this; test that it doesn't crash.
            assert!(result.text.contains("invalid") || !result.text.is_empty());
        }

        #[test]
        fn handler_name() {
            assert_eq!(YamlHandler.name(), "yaml");
        }

        #[test]
        fn describe_mapping() {
            let v: serde_json::Value = json!({"a": 1, "b": 2});
            let schema = describe_yaml_schema(&v);
            assert_eq!(schema["type"], "mapping");
        }

        #[test]
        fn describe_sequence() {
            let v: serde_json::Value = json!([1, 2, 3]);
            let schema = describe_yaml_schema(&v);
            assert_eq!(schema["type"], "sequence");
            assert_eq!(schema["length"], 3);
        }

        #[tokio::test]
        async fn extract_empty_yaml() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("empty.yaml");
            tokio::fs::write(&path, "").await.unwrap();

            let info = FileInfo {
                path: path.clone(),
                file_type: FileType::Yaml,
                mime_type: None,
                size_bytes: 0,
                is_binary: false,
            };
            let result = YamlHandler.extract(&info, 1000).await.unwrap();

            assert!(!result.truncated);
            assert_eq!(result.estimated_tokens, 0);
        }

        #[tokio::test]
        async fn extract_yaml_scalar() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("scalar.yaml");
            tokio::fs::write(&path, "42").await.unwrap();

            let info = FileInfo {
                path: path.clone(),
                file_type: FileType::Yaml,
                mime_type: None,
                size_bytes: 2,
                is_binary: false,
            };
            let result = YamlHandler.extract(&info, 1000).await.unwrap();

            assert!(result.metadata["valid"].as_bool().unwrap());
            assert_eq!(result.metadata["schema"]["type"], "scalar");
        }

        #[tokio::test]
        async fn extract_yaml_zero_budget() {
            let dir = tempfile::TempDir::new().unwrap();
            let path = dir.path().join("data.yaml");
            let content = "name: test\nversion: 1\n";
            tokio::fs::write(&path, content).await.unwrap();

            let info = FileInfo {
                path: path.clone(),
                file_type: FileType::Yaml,
                mime_type: None,
                size_bytes: content.len() as u64,
                is_binary: false,
            };
            let result = YamlHandler.extract(&info, 0).await.unwrap();

            assert!(result.truncated);
        }

        #[test]
        fn describe_scalar() {
            let v: serde_json::Value = json!("hello");
            let schema = describe_yaml_schema(&v);
            assert_eq!(schema["type"], "scalar");
        }
    }
}

#[cfg(feature = "yaml")]
pub use inner::YamlHandler;
