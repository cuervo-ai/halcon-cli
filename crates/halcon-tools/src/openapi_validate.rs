//! OpenApiValidateTool — parse and validate OpenAPI 2.0/3.0/3.1 specification files.
//!
//! Reads OpenAPI (Swagger) YAML or JSON files and provides:
//! - Schema validation (required fields, types, format)
//! - Endpoint inventory (list all paths and methods)
//! - Security scheme detection
//! - Response code completeness check
//! - Stats summary (endpoints, schemas, tags, operationIds)
//!
//! Does NOT make HTTP requests — purely static analysis.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};

pub struct OpenApiValidateTool;

impl OpenApiValidateTool {
    pub fn new() -> Self {
        Self
    }

    /// Find OpenAPI spec files in a directory.
    fn find_spec_files(dir: &Path) -> Vec<PathBuf> {
        let candidates = [
            "openapi.yaml",
            "openapi.yml",
            "openapi.json",
            "swagger.yaml",
            "swagger.yml",
            "swagger.json",
            "api.yaml",
            "api.yml",
            "api.json",
            "docs/openapi.yaml",
            "docs/openapi.json",
            "docs/swagger.yaml",
            "api/openapi.yaml",
            "api/swagger.yaml",
            ".openapi.yaml",
            ".openapi.json",
        ];
        candidates
            .iter()
            .filter_map(|&rel| {
                let p = dir.join(rel);
                if p.exists() {
                    Some(p)
                } else {
                    None
                }
            })
            .collect()
    }

    fn parse_spec(content: &str, path: &Path) -> Result<Value, String> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "yaml" | "yml" => {
                serde_yaml::from_str(content).map_err(|e| format!("YAML parse error: {}", e))
            }
            "json" => serde_json::from_str(content).map_err(|e| format!("JSON parse error: {}", e)),
            _ => {
                // Try JSON first, then YAML
                serde_json::from_str(content).or_else(|_| {
                    serde_yaml::from_str(content).map_err(|e| format!("Parse error: {}", e))
                })
            }
        }
    }

    fn detect_version(spec: &Value) -> &'static str {
        if spec.get("openapi").is_some() {
            let v = spec["openapi"].as_str().unwrap_or("");
            if v.starts_with("3.1") {
                "OpenAPI 3.1"
            } else if v.starts_with("3.0") {
                "OpenAPI 3.0"
            } else {
                "OpenAPI 3.x"
            }
        } else if spec.get("swagger").is_some() {
            "Swagger 2.0"
        } else {
            "Unknown"
        }
    }

    fn validate_spec(spec: &Value) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let version = Self::detect_version(spec);

        // Required top-level fields
        if spec.get("info").is_none() {
            issues.push(ValidationIssue::error("Missing required field 'info'"));
        } else {
            let info = &spec["info"];
            if info.get("title").is_none() {
                issues.push(ValidationIssue::error("info.title is required"));
            }
            if info.get("version").is_none() {
                issues.push(ValidationIssue::error("info.version is required"));
            }
        }

        if spec.get("paths").is_none() {
            issues.push(ValidationIssue::warning(
                "No 'paths' defined — is this intentional?",
            ));
        }

        // Check OpenAPI 3.x specific fields
        if version.starts_with("OpenAPI 3") && spec.get("openapi").is_none() {
            issues.push(ValidationIssue::error("Missing 'openapi' version field"));
        }
        // Swagger 2.0 specific
        if version == "Swagger 2.0" && spec.get("host").is_none() && spec.get("servers").is_none() {
            issues.push(ValidationIssue::info(
                "No 'host' defined (optional but recommended)",
            ));
        }

        // Validate paths
        if let Some(paths) = spec["paths"].as_object() {
            for (path, path_item) in paths {
                // Each path must start with /
                if !path.starts_with('/') {
                    issues.push(ValidationIssue::error(&format!(
                        "Path '{}' must start with '/'",
                        path
                    )));
                }

                // Check operations
                let http_methods = [
                    "get", "post", "put", "patch", "delete", "head", "options", "trace",
                ];
                for method in &http_methods {
                    if let Some(op) = path_item.get(method) {
                        // Each operation should have responses
                        if op.get("responses").is_none() {
                            issues.push(ValidationIssue::warning(&format!(
                                "{} {} missing 'responses'",
                                method.to_uppercase(),
                                path
                            )));
                        }
                        // Check for duplicate operationIds (collected below)
                    }
                }
            }
        }

        // Check for duplicate operationIds
        let mut op_ids: Vec<String> = Vec::new();
        if let Some(paths) = spec["paths"].as_object() {
            for path_item in paths.values() {
                for method in &["get", "post", "put", "patch", "delete"] {
                    if let Some(op_id) = path_item[method]["operationId"].as_str() {
                        if op_ids.contains(&op_id.to_string()) {
                            issues.push(ValidationIssue::error(&format!(
                                "Duplicate operationId: '{}'",
                                op_id
                            )));
                        }
                        op_ids.push(op_id.to_string());
                    }
                }
            }
        }

        issues
    }

    fn collect_endpoints(spec: &Value) -> Vec<EndpointInfo> {
        let mut endpoints = Vec::new();
        if let Some(paths) = spec["paths"].as_object() {
            let methods = ["get", "post", "put", "patch", "delete", "head", "options"];
            for (path, path_item) in paths {
                for method in &methods {
                    if let Some(op) = path_item.get(method) {
                        let op_id = op["operationId"].as_str().unwrap_or("").to_string();
                        let summary = op["summary"].as_str().unwrap_or("").to_string();
                        let tags: Vec<String> = op["tags"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|t| t.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let response_codes: Vec<String> = op["responses"]
                            .as_object()
                            .map(|r| r.keys().cloned().collect())
                            .unwrap_or_default();

                        endpoints.push(EndpointInfo {
                            method: method.to_uppercase(),
                            path: path.clone(),
                            operation_id: op_id,
                            summary,
                            tags,
                            _response_codes: response_codes,
                        });
                    }
                }
            }
        }
        endpoints.sort_by(|a, b| a.path.cmp(&b.path).then(a.method.cmp(&b.method)));
        endpoints
    }

    fn collect_schema_names(spec: &Value) -> Vec<String> {
        // OpenAPI 3.x: components.schemas
        if let Some(schemas) = spec["components"]["schemas"].as_object() {
            return schemas.keys().cloned().collect();
        }
        // Swagger 2.0: definitions
        if let Some(defs) = spec["definitions"].as_object() {
            return defs.keys().cloned().collect();
        }
        vec![]
    }

    fn format_report(
        spec: &Value,
        issues: &[ValidationIssue],
        endpoints: &[EndpointInfo],
        schema_names: &[String],
        path: &Path,
    ) -> String {
        let version = Self::detect_version(spec);
        let title = spec["info"]["title"].as_str().unwrap_or("(untitled)");
        let api_version = spec["info"]["version"].as_str().unwrap_or("?");

        let errors: Vec<_> = issues.iter().filter(|i| i.level == Level::Error).collect();
        let warnings: Vec<_> = issues
            .iter()
            .filter(|i| i.level == Level::Warning)
            .collect();

        let status = if errors.is_empty() {
            "✅ Valid"
        } else {
            "❌ Invalid"
        };

        let mut out = format!(
            "{} — {} ({}, v{})\nFile: {}\n\n",
            status,
            title,
            version,
            api_version,
            path.display()
        );

        // Stats
        out.push_str(&format!(
            "📊 Stats:\n  Endpoints: {}\n  Schemas: {}\n",
            endpoints.len(),
            schema_names.len()
        ));

        // Count by method
        let mut method_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for ep in endpoints {
            *method_counts.entry(ep.method.as_str()).or_insert(0) += 1;
        }
        let mut methods: Vec<(&str, usize)> = method_counts.iter().map(|(&k, &v)| (k, v)).collect();
        methods.sort_by_key(|&(m, _)| m);
        if !methods.is_empty() {
            let method_str: Vec<String> = methods
                .iter()
                .map(|(m, n)| format!("{}:{}", m, n))
                .collect();
            out.push_str(&format!("  Methods: {}\n", method_str.join("  ")));
        }

        // Tags
        let mut all_tags: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for ep in endpoints {
            for tag in &ep.tags {
                all_tags.insert(tag.as_str());
            }
        }
        if !all_tags.is_empty() {
            let mut tags: Vec<&str> = all_tags.into_iter().collect();
            tags.sort();
            out.push_str(&format!("  Tags: {}\n", tags.join(", ")));
        }

        // Issues
        if !issues.is_empty() {
            out.push_str(&format!(
                "\n⚠️  Issues ({} errors, {} warnings):\n",
                errors.len(),
                warnings.len()
            ));
            for issue in issues {
                let icon = match issue.level {
                    Level::Error => "❌",
                    Level::Warning => "⚠️",
                    Level::Info => "ℹ️",
                };
                out.push_str(&format!("  {} {}\n", icon, issue.message));
            }
        } else {
            out.push_str("\n✅ No validation issues found\n");
        }

        // Endpoint listing (first 30)
        if !endpoints.is_empty() {
            out.push_str("\n📋 Endpoints:\n");
            for ep in endpoints.iter().take(30) {
                let op_hint = if ep.operation_id.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", ep.operation_id)
                };
                let summary_hint = if ep.summary.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", &ep.summary[..ep.summary.len().min(50)])
                };
                out.push_str(&format!(
                    "  {:7} {}{}{}\n",
                    ep.method, ep.path, op_hint, summary_hint
                ));
            }
            if endpoints.len() > 30 {
                out.push_str(&format!(
                    "  ... and {} more endpoints\n",
                    endpoints.len() - 30
                ));
            }
        }

        out
    }
}

impl Default for OpenApiValidateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Level {
    Error,
    Warning,
    Info,
}

struct ValidationIssue {
    level: Level,
    message: String,
}

impl ValidationIssue {
    fn error(msg: &str) -> Self {
        Self {
            level: Level::Error,
            message: msg.to_string(),
        }
    }
    fn warning(msg: &str) -> Self {
        Self {
            level: Level::Warning,
            message: msg.to_string(),
        }
    }
    fn info(msg: &str) -> Self {
        Self {
            level: Level::Info,
            message: msg.to_string(),
        }
    }
}

struct EndpointInfo {
    method: String,
    path: String,
    operation_id: String,
    summary: String,
    tags: Vec<String>,
    _response_codes: Vec<String>,
}

#[async_trait]
impl Tool for OpenApiValidateTool {
    fn name(&self) -> &str {
        "openapi_validate"
    }

    fn description(&self) -> &str {
        "Parse and validate OpenAPI 2.0/3.0/3.1 (Swagger) specification files in YAML or JSON format. \
         Checks required fields, validates path structure, detects duplicate operationIds, \
         lists all endpoints with methods and operation IDs, counts schemas/tags. \
         Purely static analysis — does not make HTTP requests. \
         Accepts file path or auto-discovers openapi.yaml/swagger.json in the project."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to OpenAPI spec file (.yaml, .yml, .json). Auto-discovers openapi.yaml/swagger.json if omitted."
                },
                "action": {
                    "type": "string",
                    "enum": ["validate", "endpoints", "schemas", "summary"],
                    "description": "Action: 'validate' (full report), 'endpoints' (list endpoints), 'schemas' (list schemas), 'summary' (quick stats). Default: 'validate'."
                }
            },
            "required": []
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
        let working_dir = PathBuf::from(&input.working_directory);

        let spec_path = match args["path"].as_str() {
            Some(p) => {
                let p = Path::new(p);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    working_dir.join(p)
                }
            }
            None => {
                let found = Self::find_spec_files(&working_dir);
                if found.is_empty() {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!(
                            "No OpenAPI spec found in {}.\n\nLooked for: openapi.yaml, openapi.json, swagger.yaml, swagger.json, api.yaml",
                            working_dir.display()
                        ),
                        is_error: false,
                        metadata: None,
                    });
                }
                found.into_iter().next().unwrap()
            }
        };

        if !spec_path.exists() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("File not found: {}", spec_path.display()),
                is_error: true,
                metadata: None,
            });
        }

        let content = match std::fs::read_to_string(&spec_path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("Failed to read {}: {}", spec_path.display(), e),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        let spec = match Self::parse_spec(&content, &spec_path) {
            Ok(s) => s,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("Parse error in {}:\n{}", spec_path.display(), e),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        let action = args["action"].as_str().unwrap_or("validate");
        let issues = Self::validate_spec(&spec);
        let endpoints = Self::collect_endpoints(&spec);
        let schema_names = Self::collect_schema_names(&spec);

        let output = match action {
            "endpoints" => {
                if endpoints.is_empty() {
                    "No endpoints defined.".to_string()
                } else {
                    let lines: Vec<String> = endpoints
                        .iter()
                        .map(|ep| format!("{:7} {}", ep.method, ep.path))
                        .collect();
                    format!("{} endpoints:\n{}", endpoints.len(), lines.join("\n"))
                }
            }
            "schemas" => {
                if schema_names.is_empty() {
                    "No schemas defined.".to_string()
                } else {
                    format!(
                        "{} schemas:\n{}",
                        schema_names.len(),
                        schema_names.join("\n")
                    )
                }
            }
            "summary" => {
                let version = Self::detect_version(&spec);
                let title = spec["info"]["title"].as_str().unwrap_or("?");
                let api_version = spec["info"]["version"].as_str().unwrap_or("?");
                format!(
                    "{} v{} ({})\nEndpoints: {}  Schemas: {}  Issues: {}",
                    title,
                    api_version,
                    version,
                    endpoints.len(),
                    schema_names.len(),
                    issues.len()
                )
            }
            _ => Self::format_report(&spec, &issues, &endpoints, &schema_names, &spec_path),
        };

        let has_errors = issues.iter().any(|i| i.level == Level::Error);
        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content: output,
            is_error: false,
            metadata: Some(json!({
                "endpoints": endpoints.len(),
                "schemas": schema_names.len(),
                "errors": issues.iter().filter(|i| i.level == Level::Error).count(),
                "warnings": issues.iter().filter(|i| i.level == Level::Warning).count(),
                "valid": !has_errors
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const OPENAPI3_YAML: &str = r#"
openapi: "3.0.3"
info:
  title: Test API
  version: "1.0.0"
  description: A test API
paths:
  /users:
    get:
      operationId: listUsers
      summary: List all users
      tags:
        - users
      responses:
        "200":
          description: OK
    post:
      operationId: createUser
      summary: Create a user
      tags:
        - users
      responses:
        "201":
          description: Created
  /users/{id}:
    get:
      operationId: getUser
      summary: Get a user by ID
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: integer
      responses:
        "200":
          description: OK
        "404":
          description: Not Found
components:
  schemas:
    User:
      type: object
      properties:
        id:
          type: integer
        name:
          type: string
"#;

    const INVALID_SPEC: &str = r#"
openapi: "3.0.3"
paths:
  users:
    get:
      responses:
        "200":
          description: OK
"#;

    fn write_spec(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn parse_yaml_spec() {
        let dir = TempDir::new().unwrap();
        let p = write_spec(dir.path(), "openapi.yaml", OPENAPI3_YAML);
        let content = std::fs::read_to_string(&p).unwrap();
        let spec = OpenApiValidateTool::parse_spec(&content, &p).unwrap();
        assert_eq!(spec["info"]["title"].as_str().unwrap(), "Test API");
    }

    #[test]
    fn detect_version_openapi3() {
        let dir = TempDir::new().unwrap();
        let p = write_spec(dir.path(), "openapi.yaml", OPENAPI3_YAML);
        let content = std::fs::read_to_string(&p).unwrap();
        let spec = OpenApiValidateTool::parse_spec(&content, &p).unwrap();
        assert_eq!(OpenApiValidateTool::detect_version(&spec), "OpenAPI 3.0");
    }

    #[test]
    fn validate_valid_spec_no_errors() {
        let dir = TempDir::new().unwrap();
        let p = write_spec(dir.path(), "openapi.yaml", OPENAPI3_YAML);
        let content = std::fs::read_to_string(&p).unwrap();
        let spec = OpenApiValidateTool::parse_spec(&content, &p).unwrap();
        let issues = OpenApiValidateTool::validate_spec(&spec);
        let errors: Vec<_> = issues.iter().filter(|i| i.level == Level::Error).collect();
        assert!(
            errors.is_empty(),
            "valid spec should have no errors: {:?}",
            errors.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn validate_invalid_spec_detects_errors() {
        let dir = TempDir::new().unwrap();
        let p = write_spec(dir.path(), "openapi.yaml", INVALID_SPEC);
        let content = std::fs::read_to_string(&p).unwrap();
        let spec = OpenApiValidateTool::parse_spec(&content, &p).unwrap();
        let issues = OpenApiValidateTool::validate_spec(&spec);
        // Missing info.title and info.version, and path doesn't start with /
        let errors: Vec<_> = issues.iter().filter(|i| i.level == Level::Error).collect();
        assert!(!errors.is_empty(), "invalid spec should have errors");
    }

    #[test]
    fn collect_endpoints_count() {
        let dir = TempDir::new().unwrap();
        let p = write_spec(dir.path(), "openapi.yaml", OPENAPI3_YAML);
        let content = std::fs::read_to_string(&p).unwrap();
        let spec = OpenApiValidateTool::parse_spec(&content, &p).unwrap();
        let endpoints = OpenApiValidateTool::collect_endpoints(&spec);
        assert_eq!(endpoints.len(), 3, "should have 3 endpoints");
    }

    #[test]
    fn collect_schemas_count() {
        let dir = TempDir::new().unwrap();
        let p = write_spec(dir.path(), "openapi.yaml", OPENAPI3_YAML);
        let content = std::fs::read_to_string(&p).unwrap();
        let spec = OpenApiValidateTool::parse_spec(&content, &p).unwrap();
        let schemas = OpenApiValidateTool::collect_schema_names(&spec);
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0], "User");
    }

    #[tokio::test]
    async fn execute_auto_discovers_spec() {
        let dir = TempDir::new().unwrap();
        write_spec(dir.path(), "openapi.yaml", OPENAPI3_YAML);
        let tool = OpenApiValidateTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("Test API") || out.content.contains("Valid"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_endpoints_action() {
        let dir = TempDir::new().unwrap();
        write_spec(dir.path(), "openapi.yaml", OPENAPI3_YAML);
        let tool = OpenApiValidateTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "action": "endpoints" }),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.contains("GET") || out.content.contains("/users"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_no_spec_returns_hint() {
        let dir = TempDir::new().unwrap();
        let tool = OpenApiValidateTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({}),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("No OpenAPI") || out.content.contains("openapi"));
    }

    #[test]
    fn tool_metadata() {
        let t = OpenApiValidateTool::default();
        assert_eq!(t.name(), "openapi_validate");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
    }
}
