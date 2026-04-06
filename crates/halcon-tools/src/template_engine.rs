//! TemplateEngineTool — render code templates with variable substitution.
//!
//! Provides lightweight template rendering for code generation:
//! - Mustache-style `{{variable}}` substitution
//! - Conditional blocks `{{#if condition}}...{{/if}}`
//! - List iteration `{{#each items}}...{{/each}}`
//! - Built-in helpers: `{{upper}}`, `{{lower}}`, `{{snake_case}}`, `{{pascal_case}}`
//! - Template file reading or inline template
//!
//! Useful for generating boilerplate: structs, tests, endpoints, config files.

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct TemplateEngineTool;

impl TemplateEngineTool {
    pub fn new() -> Self {
        Self
    }

    /// Convert "hello_world" or "Hello World" → "HelloWorld" (PascalCase).
    fn pascal_case(s: &str) -> String {
        s.split(['_', '-', ' '])
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
                }
            })
            .collect()
    }

    /// Convert "HelloWorld" or "hello world" → "hello_world" (snake_case).
    fn snake_case(s: &str) -> String {
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if ch.is_uppercase() && i > 0 {
                out.push('_');
            }
            out.push(ch.to_lowercase().next().unwrap_or(ch));
        }
        out.replace([' ', '-'], "_")
    }

    /// Convert "hello_world" → "HELLO_WORLD" (SCREAMING_SNAKE_CASE).
    fn screaming_snake(s: &str) -> String {
        Self::snake_case(s).to_uppercase()
    }

    /// Convert "HelloWorld" → "hello-world" (kebab-case).
    fn kebab_case(s: &str) -> String {
        Self::snake_case(s).replace('_', "-")
    }

    /// Apply a helper transform to a value.
    fn apply_helper(helper: &str, value: &str) -> String {
        match helper {
            "upper" => value.to_uppercase(),
            "lower" => value.to_lowercase(),
            "pascal_case" | "pascal" => Self::pascal_case(value),
            "snake_case" | "snake" => Self::snake_case(value),
            "screaming_snake" | "UPPER_SNAKE" => Self::screaming_snake(value),
            "kebab_case" | "kebab" => Self::kebab_case(value),
            "trim" => value.trim().to_string(),
            "len" => value.len().to_string(),
            _ => value.to_string(),
        }
    }

    /// Render a template string with the given variables.
    pub fn render(template: &str, vars: &HashMap<String, Value>) -> Result<String, String> {
        let mut result = template.to_string();

        // Process {{#each list}}...{{/each}} blocks
        result = Self::process_each(&result, vars)?;

        // Process {{#if var}}...{{/if}} blocks
        result = Self::process_if(&result, vars)?;

        // Process simple {{var}} and {{helper var}} substitutions
        result = Self::process_vars(&result, vars)?;

        Ok(result)
    }

    fn process_vars(template: &str, vars: &HashMap<String, Value>) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = template;

        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            rest = &rest[start + 2..];
            let end = rest
                .find("}}")
                .ok_or_else(|| "Unclosed {{ in template".to_string())?;
            let expr = rest[..end].trim();
            rest = &rest[end + 2..];

            // Skip block directives (already processed)
            if expr.starts_with('#') || expr.starts_with('/') {
                out.push_str(&format!("{{{{{expr}}}}}"));
                continue;
            }

            // Check for helper: "upper name" → apply_helper("upper", value_of_name)
            let parts: Vec<&str> = expr.splitn(2, ' ').collect();
            let value = if parts.len() == 2 {
                let helper = parts[0];
                let var_name = parts[1];
                let raw = Self::resolve_var(vars, var_name);
                Self::apply_helper(helper, &raw)
            } else {
                Self::resolve_var(vars, expr)
            };

            out.push_str(&value);
        }
        out.push_str(rest);
        Ok(out)
    }

    fn resolve_var(vars: &HashMap<String, Value>, name: &str) -> String {
        // Support dot notation: "user.name"
        let parts: Vec<&str> = name.splitn(2, '.').collect();
        if parts.len() == 2 {
            if let Some(obj) = vars.get(parts[0]).and_then(|v| v.as_object()) {
                return obj
                    .get(parts[1])
                    .map(Self::value_to_string)
                    .unwrap_or_default();
            }
        }
        vars.get(name)
            .map(Self::value_to_string)
            .unwrap_or_default()
    }

    fn value_to_string(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            _ => serde_json::to_string(v).unwrap_or_default(),
        }
    }

    fn process_if(template: &str, vars: &HashMap<String, Value>) -> Result<String, String> {
        let mut result = template.to_string();
        let mut iteration = 0;

        loop {
            iteration += 1;
            if iteration > 100 {
                return Err("Too many nested if blocks".to_string());
            }

            if let Some(start) = result.find("{{#if ") {
                let tag_end = result[start..].find("}}").ok_or("Unclosed {{#if")? + start;
                let var_name = result[start + 6..tag_end].trim();
                let close_tag = "{{/if}}".to_string();
                let close_pos = result[tag_end..]
                    .find(&close_tag)
                    .ok_or_else(|| format!("Missing {{{{/if}}}} for {{{{#if {var_name}}}}}"))?
                    + tag_end;

                let inner = &result[tag_end + 2..close_pos];
                let is_truthy = {
                    let val = vars.get(var_name);
                    match val {
                        None => false,
                        Some(Value::Bool(b)) => *b,
                        Some(Value::Null) => false,
                        Some(Value::String(s)) => !s.is_empty(),
                        Some(Value::Array(a)) => !a.is_empty(),
                        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
                        _ => true,
                    }
                };

                let replacement = if is_truthy {
                    inner.to_string()
                } else {
                    String::new()
                };
                result = format!(
                    "{}{}{}",
                    &result[..start],
                    replacement,
                    &result[close_pos + close_tag.len()..]
                );
            } else {
                break;
            }
        }
        Ok(result)
    }

    fn process_each(template: &str, vars: &HashMap<String, Value>) -> Result<String, String> {
        let mut result = template.to_string();
        let mut iteration = 0;

        loop {
            iteration += 1;
            if iteration > 100 {
                return Err("Too many nested each blocks".to_string());
            }

            if let Some(start) = result.find("{{#each ") {
                let tag_end = result[start..].find("}}").ok_or("Unclosed {{#each")? + start;
                let list_name = result[start + 8..tag_end].trim();
                let close_tag = "{{/each}}";
                let close_pos = result[tag_end..].find(close_tag).ok_or_else(|| {
                    format!("Missing {{{{/each}}}} for {{{{#each {list_name}}}}}")
                })? + tag_end;

                let item_template = &result[tag_end + 2..close_pos];
                let items = vars.get(list_name).and_then(|v| v.as_array());

                let rendered = if let Some(arr) = items {
                    arr.iter()
                        .map(|item| {
                            // Make item available as "this" or its string value as "."
                            let mut item_vars = vars.clone();
                            item_vars.insert("this".to_string(), item.clone());
                            item_vars.insert(".".to_string(), item.clone());
                            // If item is object, merge its keys
                            if let Some(obj) = item.as_object() {
                                for (k, v) in obj {
                                    item_vars.insert(k.clone(), v.clone());
                                }
                            }
                            // Process nested (only vars, not blocks — no recursion here)
                            Self::process_vars(item_template, &item_vars)
                                .unwrap_or_else(|_| item_template.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join("")
                } else {
                    String::new()
                };

                result = format!(
                    "{}{}{}",
                    &result[..start],
                    rendered,
                    &result[close_pos + close_tag.len()..]
                );
            } else {
                break;
            }
        }
        Ok(result)
    }
}

impl Default for TemplateEngineTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TemplateEngineTool {
    fn name(&self) -> &str {
        "template_engine"
    }

    fn description(&self) -> &str {
        "Render code templates with variable substitution. \
         Supports Mustache-style {{variable}} interpolation, {{#if condition}}...{{/if}} conditionals, \
         {{#each list}}...{{/each}} iteration, and built-in helpers: upper, lower, pascal_case, \
         snake_case, screaming_snake, kebab_case. \
         Templates can be inline strings or read from files. \
         Useful for generating boilerplate code: structs, tests, REST endpoints, config files."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "template": {
                    "type": "string",
                    "description": "Template string with {{variable}} placeholders."
                },
                "template_file": {
                    "type": "string",
                    "description": "Path to a template file (alternative to 'template')."
                },
                "vars": {
                    "type": "object",
                    "description": "Variables to substitute. Values can be strings, numbers, booleans, or arrays."
                },
                "output_file": {
                    "type": "string",
                    "description": "If provided, write rendered output to this file."
                }
            },
            "required": []
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn requires_confirmation(&self, input: &halcon_core::types::ToolInput) -> bool {
        // Only requires confirmation if writing to a file
        input.arguments["output_file"].as_str().is_some()
    }

    async fn execute_inner(
        &self,
        input: ToolInput,
    ) -> Result<ToolOutput, halcon_core::error::HalconError> {
        let args = &input.arguments;

        // Get template source
        let template = if let Some(tmpl) = args["template"].as_str() {
            tmpl.to_string()
        } else if let Some(path) = args["template_file"].as_str() {
            match tokio::fs::read_to_string(path).await {
                Ok(content) => content,
                Err(e) => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!("Failed to read template file '{path}': {e}"),
                        is_error: true,
                        metadata: None,
                    })
                }
            }
        } else {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "Provide 'template' (inline string) or 'template_file' (path)."
                    .to_string(),
                is_error: true,
                metadata: None,
            });
        };

        // Collect vars
        let vars: HashMap<String, Value> = args["vars"]
            .as_object()
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        // Render
        let rendered = match Self::render(&template, &vars) {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("Template rendering error: {e}"),
                    is_error: true,
                    metadata: None,
                })
            }
        };

        // Optionally write to file
        let wrote_file = if let Some(out_path) = args["output_file"].as_str() {
            match tokio::fs::write(out_path, &rendered).await {
                Ok(_) => true,
                Err(e) => {
                    return Ok(ToolOutput {
                        tool_use_id: input.tool_use_id,
                        content: format!("Failed to write to '{out_path}': {e}"),
                        is_error: true,
                        metadata: None,
                    })
                }
            }
        } else {
            false
        };

        let lines = rendered.lines().count();
        let content = if wrote_file {
            format!(
                "Rendered {} lines to '{}'\n\n```\n{}\n```",
                lines,
                args["output_file"].as_str().unwrap_or(""),
                if lines > 50 {
                    rendered.lines().take(50).collect::<Vec<_>>().join("\n") + "\n..."
                } else {
                    rendered.clone()
                }
            )
        } else {
            rendered.clone()
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "lines": lines,
                "vars_used": vars.len(),
                "wrote_file": wrote_file
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn make_vars(pairs: &[(&str, &str)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    fn make_input(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t1".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn tool_metadata() {
        let t = TemplateEngineTool::default();
        assert_eq!(t.name(), "template_engine");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn pascal_case_conversions() {
        assert_eq!(TemplateEngineTool::pascal_case("hello_world"), "HelloWorld");
        assert_eq!(
            TemplateEngineTool::pascal_case("my-component"),
            "MyComponent"
        );
        assert_eq!(TemplateEngineTool::pascal_case("foo"), "Foo");
    }

    #[test]
    fn snake_case_conversions() {
        assert_eq!(TemplateEngineTool::snake_case("HelloWorld"), "hello_world");
        assert_eq!(TemplateEngineTool::snake_case("myVar"), "my_var");
        assert_eq!(TemplateEngineTool::snake_case("foo"), "foo");
    }

    #[test]
    fn kebab_case_conversions() {
        assert_eq!(TemplateEngineTool::kebab_case("HelloWorld"), "hello-world");
        assert_eq!(TemplateEngineTool::kebab_case("my_var"), "my-var");
    }

    #[test]
    fn screaming_snake_conversions() {
        assert_eq!(TemplateEngineTool::screaming_snake("myVar"), "MY_VAR");
        assert_eq!(
            TemplateEngineTool::screaming_snake("hello_world"),
            "HELLO_WORLD"
        );
    }

    #[test]
    fn simple_substitution() {
        let vars = make_vars(&[("name", "Alice"), ("lang", "Rust")]);
        let result = TemplateEngineTool::render("Hello {{name}} from {{lang}}!", &vars).unwrap();
        assert_eq!(result, "Hello Alice from Rust!");
    }

    #[test]
    fn helper_upper() {
        let vars = make_vars(&[("name", "hello")]);
        let result = TemplateEngineTool::render("{{upper name}}", &vars).unwrap();
        assert_eq!(result, "HELLO");
    }

    #[test]
    fn helper_pascal_case() {
        let vars = make_vars(&[("name", "my_struct")]);
        let result = TemplateEngineTool::render("{{pascal_case name}}", &vars).unwrap();
        assert_eq!(result, "MyStruct");
    }

    #[test]
    fn if_block_true() {
        let mut vars = HashMap::new();
        vars.insert("is_async".to_string(), Value::Bool(true));
        let result =
            TemplateEngineTool::render("{{#if is_async}}async {{/if}}fn main() {}", &vars).unwrap();
        assert_eq!(result, "async fn main() {}");
    }

    #[test]
    fn if_block_false() {
        let mut vars = HashMap::new();
        vars.insert("is_async".to_string(), Value::Bool(false));
        let result =
            TemplateEngineTool::render("{{#if is_async}}async {{/if}}fn main() {}", &vars).unwrap();
        assert_eq!(result, "fn main() {}");
    }

    #[test]
    fn each_block_renders_list() {
        let mut vars = HashMap::new();
        vars.insert("fields".to_string(), json!(["name", "age", "email"]));
        let result =
            TemplateEngineTool::render("{{#each fields}}  {{this}}\n{{/each}}", &vars).unwrap();
        assert!(result.contains("name"));
        assert!(result.contains("age"));
        assert!(result.contains("email"));
    }

    #[test]
    fn each_block_with_objects() {
        let mut vars = HashMap::new();
        vars.insert(
            "items".to_string(),
            json!([
                { "name": "Alice", "role": "admin" },
                { "name": "Bob", "role": "user" }
            ]),
        );
        let result =
            TemplateEngineTool::render("{{#each items}}{{name}}:{{role}} {{/each}}", &vars)
                .unwrap();
        assert!(result.contains("Alice:admin"));
        assert!(result.contains("Bob:user"));
    }

    #[test]
    fn missing_var_renders_empty() {
        let vars = HashMap::new();
        let result = TemplateEngineTool::render("Hello {{name}}!", &vars).unwrap();
        assert_eq!(result, "Hello !");
    }

    #[test]
    fn unclosed_tag_returns_error() {
        let vars = HashMap::new();
        let result = TemplateEngineTool::render("Hello {{name", &vars);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_inline_template() {
        let tool = TemplateEngineTool::new();
        let out = tool
            .execute(make_input(json!({
                "template": "fn {{snake_case name}}() -> {{return_type}} {\n    todo!()\n}",
                "vars": { "name": "MyFunction", "return_type": "String" }
            })))
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        assert!(
            out.content.contains("my_function"),
            "content: {}",
            out.content
        );
        assert!(out.content.contains("String"), "content: {}", out.content);
    }

    #[tokio::test]
    async fn execute_no_template_returns_error() {
        let tool = TemplateEngineTool::new();
        let out = tool.execute(make_input(json!({}))).await.unwrap();
        assert!(out.is_error);
    }
}
