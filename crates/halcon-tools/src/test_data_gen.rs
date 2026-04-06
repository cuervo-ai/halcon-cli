//! TestDataGenTool — generate realistic test fixtures and mock data.
//!
//! Generates:
//! - JSON objects following a schema spec
//! - CSV rows with typed columns
//! - SQL INSERT statements
//! - UUIDs, timestamps, and random values
//! - Realistic names, emails, URLs, and IPs

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct TestDataGenTool;

impl TestDataGenTool {
    pub fn new() -> Self {
        Self
    }

    /// Simple deterministic pseudo-random from a seed (LCG).
    fn lcg(seed: u64) -> u64 {
        seed.wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
    }

    fn rand_range(seed: &mut u64, min: u64, max: u64) -> u64 {
        *seed = Self::lcg(*seed);
        if max <= min {
            return min;
        }
        min + (*seed % (max - min))
    }

    fn rand_f64(seed: &mut u64) -> f64 {
        *seed = Self::lcg(*seed);
        (*seed as f64) / (u64::MAX as f64)
    }

    fn pick<'a, T>(seed: &mut u64, items: &'a [T]) -> &'a T {
        let idx = Self::rand_range(seed, 0, items.len() as u64) as usize;
        &items[idx]
    }

    // Realistic data pools
    fn first_names() -> &'static [&'static str] {
        &[
            "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Henry", "Isabel", "Jack",
            "Karen", "Liam", "Maria", "Nathan", "Olivia", "Peter", "Quinn", "Rachel", "Sam",
            "Tina",
        ]
    }

    fn last_names() -> &'static [&'static str] {
        &[
            "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis",
            "Wilson", "Anderson", "Taylor", "Thomas", "Jackson", "White", "Harris", "Martin",
        ]
    }

    fn domains() -> &'static [&'static str] {
        &[
            "example.com",
            "test.org",
            "mock.io",
            "sample.net",
            "demo.dev",
            "fake.co",
        ]
    }

    fn words() -> &'static [&'static str] {
        &[
            "apple", "bravo", "cloud", "delta", "echo", "foxtrot", "golf", "hotel", "india",
            "juliet", "kilo", "lima", "mike", "november", "oscar", "papa",
        ]
    }

    fn gen_uuid(seed: &mut u64) -> String {
        let a = Self::rand_range(seed, 0, u32::MAX as u64) as u32;
        let b = Self::rand_range(seed, 0, u16::MAX as u64) as u16;
        let c = (Self::rand_range(seed, 0, u16::MAX as u64) as u16 & 0x0FFF) | 0x4000;
        let d = (Self::rand_range(seed, 0, u16::MAX as u64) as u16 & 0x3FFF) | 0x8000;
        let e = Self::rand_range(seed, 0, u64::MAX) & 0xFFFF_FFFF_FFFF;
        format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", a, b, c, d, e)
    }

    fn gen_timestamp(seed: &mut u64) -> String {
        // Dates between 2020-01-01 and 2024-12-31
        let start = 1577836800u64; // 2020-01-01
        let end = 1735689600u64; // 2025-01-01
        let ts = Self::rand_range(seed, start, end);
        // Simple format: convert to YYYY-MM-DD HH:MM:SS (approximate, no timezone)
        let secs = ts;
        let days = secs / 86400;
        let time_of_day = secs % 86400;
        let h = time_of_day / 3600;
        let m = (time_of_day % 3600) / 60;
        let s = time_of_day % 60;
        // Very rough date from days since epoch
        let year = 1970 + days / 365;
        let day_of_year = days % 365;
        let month = day_of_year / 30 + 1;
        let day = day_of_year % 30 + 1;
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            year.min(2024),
            month.min(12),
            day.min(28),
            h,
            m,
            s
        )
    }

    fn gen_email(seed: &mut u64) -> String {
        let first = Self::pick(seed, Self::first_names()).to_lowercase();
        let last = Self::pick(seed, Self::last_names()).to_lowercase();
        let domain = Self::pick(seed, Self::domains());
        format!("{}.{}@{}", first, last, domain)
    }

    fn gen_name(seed: &mut u64) -> String {
        let first = Self::pick(seed, Self::first_names());
        let last = Self::pick(seed, Self::last_names());
        format!("{} {}", first, last)
    }

    fn gen_ip(seed: &mut u64) -> String {
        let a = Self::rand_range(seed, 1, 254);
        let b = Self::rand_range(seed, 0, 255);
        let c = Self::rand_range(seed, 0, 255);
        let d = Self::rand_range(seed, 1, 254);
        format!("{}.{}.{}.{}", a, b, c, d)
    }

    fn gen_url(seed: &mut u64) -> String {
        let domain = Self::pick(seed, Self::domains());
        let path = Self::pick(seed, Self::words());
        format!("https://{}/{}", domain, path)
    }

    fn gen_phone(seed: &mut u64) -> String {
        let area = Self::rand_range(seed, 200, 999);
        let prefix = Self::rand_range(seed, 200, 999);
        let line = Self::rand_range(seed, 1000, 9999);
        format!("+1-{}-{}-{}", area, prefix, line)
    }

    /// Generate a value matching a field spec string like "name", "email", "integer:1:100", etc.
    fn gen_field_value(field_type: &str, idx: usize, seed: &mut u64) -> Value {
        let parts: Vec<&str> = field_type.splitn(3, ':').collect();
        match parts[0].trim() {
            "uuid" => json!(Self::gen_uuid(seed)),
            "id" => json!(idx + 1),
            "integer" | "int" => {
                let min = parts
                    .get(1)
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1);
                let max = parts
                    .get(2)
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1000);
                json!(Self::rand_range(seed, min, max))
            }
            "float" | "number" => {
                let min: f64 = parts.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                let max: f64 = parts.get(2).and_then(|v| v.parse().ok()).unwrap_or(1.0);
                let v = min + Self::rand_f64(seed) * (max - min);
                json!(format!("{:.2}", v).parse::<f64>().unwrap_or(v))
            }
            "bool" | "boolean" => json!(Self::rand_range(seed, 0, 2) == 0),
            "name" | "fullname" => json!(Self::gen_name(seed)),
            "firstname" => json!(Self::pick(seed, Self::first_names()).to_string()),
            "lastname" => json!(Self::pick(seed, Self::last_names()).to_string()),
            "email" => json!(Self::gen_email(seed)),
            "timestamp" | "datetime" => json!(Self::gen_timestamp(seed)),
            "date" => json!(&Self::gen_timestamp(seed)[..10]),
            "ip" => json!(Self::gen_ip(seed)),
            "url" | "uri" => json!(Self::gen_url(seed)),
            "phone" => json!(Self::gen_phone(seed)),
            "word" => json!(Self::pick(seed, Self::words()).to_string()),
            "status" => {
                let statuses = ["active", "inactive", "pending", "suspended"];
                json!(Self::pick(seed, &statuses).to_string())
            }
            "color" => {
                let r = Self::rand_range(seed, 0, 256);
                let g = Self::rand_range(seed, 0, 256);
                let b = Self::rand_range(seed, 0, 256);
                json!(format!("#{:02X}{:02X}{:02X}", r, g, b))
            }
            s => json!(format!("{}-{}", s, idx + 1)),
        }
    }

    fn generate_json(schema: &HashMap<String, String>, count: usize, seed_base: u64) -> String {
        let mut records = vec![];
        for i in 0..count {
            let mut seed = seed_base.wrapping_add(i as u64 * 997);
            let mut obj = serde_json::Map::new();
            for (field, field_type) in schema {
                let val = Self::gen_field_value(field_type, i, &mut seed);
                obj.insert(field.clone(), val);
            }
            records.push(Value::Object(obj));
        }
        serde_json::to_string_pretty(&Value::Array(records)).unwrap_or_default()
    }

    fn generate_csv(schema: &[(String, String)], count: usize, seed_base: u64) -> String {
        let mut out = String::new();
        // Header
        let headers: Vec<&str> = schema.iter().map(|(k, _)| k.as_str()).collect();
        out.push_str(&headers.join(","));
        out.push('\n');
        // Rows
        for i in 0..count {
            let mut seed = seed_base.wrapping_add(i as u64 * 997);
            let values: Vec<String> = schema
                .iter()
                .map(|(_, t)| {
                    let v = Self::gen_field_value(t, i, &mut seed);
                    match &v {
                        Value::String(s) => {
                            if s.contains(',') || s.contains('"') {
                                format!("\"{}\"", s.replace('"', "\"\""))
                            } else {
                                s.clone()
                            }
                        }
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => v.to_string(),
                    }
                })
                .collect();
            out.push_str(&values.join(","));
            out.push('\n');
        }
        out
    }

    fn generate_sql(
        table: &str,
        schema: &[(String, String)],
        count: usize,
        seed_base: u64,
    ) -> String {
        let mut out = String::new();
        let cols: Vec<&str> = schema.iter().map(|(k, _)| k.as_str()).collect();
        let cols_str = cols.join(", ");

        for i in 0..count {
            let mut seed = seed_base.wrapping_add(i as u64 * 997);
            let values: Vec<String> = schema
                .iter()
                .map(|(_, t)| {
                    let v = Self::gen_field_value(t, i, &mut seed);
                    match &v {
                        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => {
                            if *b {
                                "TRUE".to_string()
                            } else {
                                "FALSE".to_string()
                            }
                        }
                        Value::Null => "NULL".to_string(),
                        _ => format!("'{}'", v.to_string().replace('\'', "''")),
                    }
                })
                .collect();
            let vals_str = values.join(", ");
            out.push_str(&format!(
                "INSERT INTO {} ({}) VALUES ({});\n",
                table, cols_str, vals_str
            ));
        }
        out
    }

    fn parse_schema_from_value(schema_val: &Value) -> Vec<(String, String)> {
        let mut fields = vec![];
        if let Some(obj) = schema_val.as_object() {
            for (k, v) in obj {
                let field_type = v.as_str().unwrap_or("word").to_string();
                fields.push((k.clone(), field_type));
            }
        }
        fields
    }
}

impl Default for TestDataGenTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TestDataGenTool {
    fn name(&self) -> &str {
        "test_data_gen"
    }

    fn description(&self) -> &str {
        "Generate realistic test fixtures and mock data. Supports JSON objects, CSV rows, \
         and SQL INSERT statements. Define field schemas with types like uuid, name, email, \
         timestamp, integer, float, ip, url, status, and more. Deterministic seeding for \
         reproducible data."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "format": {
                    "type": "string",
                    "enum": ["json", "csv", "sql"],
                    "description": "Output format (default: json)."
                },
                "count": {
                    "type": "integer",
                    "description": "Number of records to generate (default: 10, max: 1000)."
                },
                "schema": {
                    "type": "object",
                    "description": "Field definitions: {fieldName: fieldType}. Types: uuid, id, integer:min:max, float:min:max, bool, name, email, timestamp, date, ip, url, phone, word, status, color."
                },
                "table": {
                    "type": "string",
                    "description": "Table name for SQL output (default: test_data)."
                },
                "seed": {
                    "type": "integer",
                    "description": "Random seed for reproducible output (default: 42)."
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
        let format = args["format"].as_str().unwrap_or("json");
        let count = args["count"].as_u64().unwrap_or(10).clamp(1, 1000) as usize;
        let table = args["table"].as_str().unwrap_or("test_data");
        let seed_base = args["seed"].as_u64().unwrap_or(42);

        // Default schema if none provided
        let schema_val = if args["schema"].is_object() {
            args["schema"].clone()
        } else {
            json!({
                "id": "id",
                "name": "name",
                "email": "email",
                "created_at": "timestamp",
                "active": "bool"
            })
        };

        let fields = Self::parse_schema_from_value(&schema_val);

        if fields.is_empty() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: "Schema is empty or invalid. Provide an object like {\"name\": \"name\", \"age\": \"integer:18:90\"}.".into(),
                is_error: true,
                metadata: None,
            });
        }

        let content = match format {
            "csv" => Self::generate_csv(&fields, count, seed_base),
            "sql" => Self::generate_sql(table, &fields, count, seed_base),
            _ => {
                // JSON: use HashMap for consistent ordering
                let schema_map: HashMap<String, String> = fields.into_iter().collect();
                Self::generate_json(&schema_map, count, seed_base)
            }
        };

        let content = if content.is_empty() {
            format!("Generated 0 records (format: {format}).")
        } else {
            content
        };

        Ok(ToolOutput {
            tool_use_id: input.tool_use_id,
            content,
            is_error: false,
            metadata: Some(json!({
                "format": format,
                "count": count,
                "seed": seed_base
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(args: Value) -> ToolInput {
        ToolInput {
            tool_use_id: "t1".into(),
            arguments: args,
            working_directory: "/tmp".into(),
        }
    }

    #[test]
    fn tool_metadata() {
        let t = TestDataGenTool::new();
        assert_eq!(t.name(), "test_data_gen");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn gen_uuid_format() {
        let mut seed = 12345u64;
        let uuid = TestDataGenTool::gen_uuid(&mut seed);
        let parts: Vec<&str> = uuid.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[2].chars().next().unwrap(), '4'); // version 4
    }

    #[test]
    fn gen_email_format() {
        let mut seed = 99u64;
        let email = TestDataGenTool::gen_email(&mut seed);
        assert!(email.contains('@'));
        assert!(email.contains('.'));
    }

    #[test]
    fn gen_ip_format() {
        let mut seed = 777u64;
        let ip = TestDataGenTool::gen_ip(&mut seed);
        let parts: Vec<&str> = ip.split('.').collect();
        assert_eq!(parts.len(), 4);
        for p in &parts {
            let n: u32 = p.parse().unwrap();
            assert!(n <= 254);
        }
    }

    #[test]
    fn deterministic_with_same_seed() {
        let schema = [
            ("name".to_string(), "name".to_string()),
            ("email".to_string(), "email".to_string()),
        ];
        let r1 = TestDataGenTool::generate_csv(&schema, 3, 42);
        let r2 = TestDataGenTool::generate_csv(&schema, 3, 42);
        assert_eq!(r1, r2);
    }

    #[test]
    fn different_seeds_produce_different_data() {
        let schema = [("name".to_string(), "name".to_string())];
        let r1 = TestDataGenTool::generate_csv(&schema, 5, 1);
        let r2 = TestDataGenTool::generate_csv(&schema, 5, 999);
        assert_ne!(r1, r2);
    }

    #[test]
    fn csv_has_correct_row_count() {
        let schema = [
            ("id".to_string(), "id".to_string()),
            ("name".to_string(), "name".to_string()),
        ];
        let csv = TestDataGenTool::generate_csv(&schema, 5, 42);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 6); // 1 header + 5 rows
        assert!(lines[0].contains("id"));
        assert!(lines[0].contains("name"));
    }

    #[test]
    fn sql_output_has_inserts() {
        let schema = [
            ("id".to_string(), "id".to_string()),
            ("email".to_string(), "email".to_string()),
        ];
        let sql = TestDataGenTool::generate_sql("users", &schema, 3, 42);
        assert_eq!(sql.lines().count(), 3);
        assert!(sql.contains("INSERT INTO users"));
    }

    #[tokio::test]
    async fn execute_default_json() {
        let tool = TestDataGenTool::new();
        let out = tool
            .execute(make_input(json!({ "count": 5 })))
            .await
            .unwrap();
        assert!(!out.is_error);
        let v: Value = serde_json::from_str(&out.content).expect("valid JSON");
        assert_eq!(v.as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn execute_csv_format() {
        let tool = TestDataGenTool::new();
        let out = tool
            .execute(make_input(json!({
                "format": "csv",
                "count": 3,
                "schema": { "id": "id", "name": "name", "email": "email" }
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines.len(), 4); // header + 3 rows
    }

    #[tokio::test]
    async fn execute_sql_format() {
        let tool = TestDataGenTool::new();
        let out = tool
            .execute(make_input(json!({
                "format": "sql",
                "count": 2,
                "table": "items",
                "schema": { "id": "id", "name": "word" }
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("INSERT INTO items"));
        assert_eq!(out.content.lines().count(), 2);
    }

    #[tokio::test]
    async fn execute_custom_schema_types() {
        let tool = TestDataGenTool::new();
        let out = tool
            .execute(make_input(json!({
                "count": 1,
                "schema": {
                    "uuid": "uuid",
                    "age": "integer:18:65",
                    "score": "float:0:100",
                    "active": "bool",
                    "status": "status",
                    "ip": "ip",
                    "url": "url"
                }
            })))
            .await
            .unwrap();
        assert!(!out.is_error);
        let v: Value = serde_json::from_str(&out.content).unwrap();
        let rec = &v[0];
        // uuid should look like a UUID
        assert!(rec["uuid"].as_str().unwrap().contains('-'));
        // status should be one of the valid values
        let valid_statuses = ["active", "inactive", "pending", "suspended"];
        assert!(valid_statuses.contains(&rec["status"].as_str().unwrap()));
    }
}
