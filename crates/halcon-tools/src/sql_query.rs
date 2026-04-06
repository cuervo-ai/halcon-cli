//! SqlQueryTool — execute read-only SQL queries against SQLite database files.
//!
//! Provides safe, read-only SQL execution against SQLite databases.
//! Only SELECT, PRAGMA (read-only), and EXPLAIN queries are allowed.
//! Useful for inspecting application databases, migrations, and data during development.
//!
//! Write operations (INSERT, UPDATE, DELETE, DROP, CREATE, ALTER) are rejected.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use halcon_core::{
    traits::Tool,
    types::{PermissionLevel, ToolInput, ToolOutput},
};
use serde_json::{json, Value};

/// Maximum number of result rows to return.
const MAX_ROWS: usize = 500;
/// Maximum cell width in formatted output.
const MAX_CELL_WIDTH: usize = 80;

pub struct SqlQueryTool;

impl SqlQueryTool {
    pub fn new() -> Self {
        Self
    }

    /// Find SQLite database files in a directory.
    fn find_db_files(dir: &Path) -> Vec<PathBuf> {
        let extensions = ["db", "sqlite", "sqlite3", "db3"];
        let skip_dirs = [".git", "target", "node_modules", ".venv"];

        let mut files = Vec::new();
        let mut stack = vec![dir.to_path_buf()];

        while let Some(d) = stack.pop() {
            if files.len() >= 20 {
                break;
            }
            if let Ok(entries) = std::fs::read_dir(&d) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if skip_dirs.contains(&name) {
                        continue;
                    }
                    if path.is_dir() {
                        stack.push(path);
                    } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if extensions.contains(&ext) {
                            files.push(path);
                        }
                    }
                }
            }
        }
        files
    }

    /// Execute a SQL query on the given database file (sync, called via spawn_blocking).
    fn execute_query_sync(db_path: &Path, sql: &str) -> Result<QueryResult, String> {
        // Open in read-only mode
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("Failed to open database: {}", e))?;

        // Set a query timeout (5s max via busy_timeout)
        conn.execute_batch("PRAGMA busy_timeout=5000;")
            .map_err(|e| format!("PRAGMA failed: {}", e))?;

        let mut stmt = conn.prepare(sql).map_err(|e| format!("SQL error: {}", e))?;

        let col_count = stmt.column_count();
        let col_names: Vec<String> = (0..col_count)
            .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
            .collect();

        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut truncated = false;

        let mut query_rows = stmt.query([]).map_err(|e| format!("Query error: {}", e))?;

        while let Some(row) = query_rows.next().map_err(|e| format!("Row error: {}", e))? {
            if rows.len() >= MAX_ROWS {
                truncated = true;
                break;
            }
            let cells: Vec<String> = (0..col_count)
                .map(|i| match row.get_ref(i) {
                    Ok(rusqlite::types::ValueRef::Null) => "NULL".to_string(),
                    Ok(rusqlite::types::ValueRef::Integer(n)) => n.to_string(),
                    Ok(rusqlite::types::ValueRef::Real(f)) => format!("{:.6}", f),
                    Ok(rusqlite::types::ValueRef::Text(t)) => {
                        let s = String::from_utf8_lossy(t).to_string();
                        if s.len() > MAX_CELL_WIDTH {
                            format!("{}...", &s[..MAX_CELL_WIDTH])
                        } else {
                            s
                        }
                    }
                    Ok(rusqlite::types::ValueRef::Blob(b)) => {
                        format!("<BLOB {} bytes>", b.len())
                    }
                    Err(e) => format!("<error: {}>", e),
                })
                .collect();
            rows.push(cells);
        }

        Ok(QueryResult {
            columns: col_names,
            rows,
            truncated,
        })
    }

    /// Format query results as a text table.
    fn format_table(result: &QueryResult) -> String {
        if result.rows.is_empty() {
            return "(no rows)\n".to_string();
        }

        // Compute column widths
        let mut widths: Vec<usize> = result.columns.iter().map(|c| c.len()).collect();
        for row in &result.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }

        // Cap column widths
        let widths: Vec<usize> = widths.iter().map(|&w| w.min(MAX_CELL_WIDTH)).collect();

        let sep: String = widths
            .iter()
            .map(|&w| "-".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("+");
        let sep = format!("+{}+", sep);

        let mut out = String::new();
        out.push_str(&sep);
        out.push('\n');

        // Header
        let header: String = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    " {:width$} ",
                    c,
                    width = widths.get(i).copied().unwrap_or(10)
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        out.push_str(&format!("|{}|\n", header));
        out.push_str(&sep);
        out.push('\n');

        // Rows
        for row in &result.rows {
            let line: String = row
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    let w = widths.get(i).copied().unwrap_or(10);
                    let truncated = if cell.len() > w {
                        &cell[..w]
                    } else {
                        cell.as_str()
                    };
                    format!(" {:width$} ", truncated, width = w)
                })
                .collect::<Vec<_>>()
                .join("|");
            out.push_str(&format!("|{}|\n", line));
        }
        out.push_str(&sep);
        out.push('\n');

        out
    }
}

impl Default for SqlQueryTool {
    fn default() -> Self {
        Self::new()
    }
}

struct QueryResult {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    truncated: bool,
}

#[async_trait]
impl Tool for SqlQueryTool {
    fn name(&self) -> &str {
        "sql_query"
    }

    fn description(&self) -> &str {
        "Execute read-only SQL queries against SQLite database files. \
         Supports SELECT, PRAGMA, EXPLAIN, and WITH (CTE) queries. \
         Write operations (INSERT, UPDATE, DELETE, DROP, CREATE, ALTER) are rejected for safety. \
         Useful for inspecting application databases, schema exploration, and data analysis during development. \
         Can list available databases in the project or describe table schemas."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "database": {
                    "type": "string",
                    "description": "Path to SQLite database file. If omitted, lists available .db/.sqlite files in the working directory."
                },
                "query": {
                    "type": "string",
                    "description": "SQL query to execute. Must be read-only (SELECT, PRAGMA, EXPLAIN). Write operations are rejected."
                },
                "action": {
                    "type": "string",
                    "enum": ["query", "schema", "tables", "list_dbs"],
                    "description": "Action: 'query' (run SQL), 'schema' (show all table schemas), 'tables' (list table names), 'list_dbs' (find database files). Default: 'query'."
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

        let action = args["action"].as_str().unwrap_or("query");

        // List databases action
        if action == "list_dbs" || (args["database"].is_null() && args["query"].is_null()) {
            let dbs = Self::find_db_files(&working_dir);
            if dbs.is_empty() {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!(
                        "No SQLite database files found in {}",
                        working_dir.display()
                    ),
                    is_error: false,
                    metadata: None,
                });
            }
            let list: Vec<String> = dbs
                .iter()
                .map(|p| {
                    p.strip_prefix(&working_dir)
                        .unwrap_or(p)
                        .to_string_lossy()
                        .to_string()
                })
                .collect();
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("SQLite databases found:\n{}", list.join("\n")),
                is_error: false,
                metadata: Some(json!({ "databases": list })),
            });
        }

        let db_path = match args["database"].as_str() {
            Some(p) => {
                let p = Path::new(p);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    working_dir.join(p)
                }
            }
            None => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: "The 'database' field is required. Use action='list_dbs' to find database files.".to_string(),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        if !db_path.exists() {
            return Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("Database file not found: {}", db_path.display()),
                is_error: true,
                metadata: None,
            });
        }

        let sql = match action {
            "tables" => "SELECT name, type FROM sqlite_master WHERE type IN ('table','view') ORDER BY type, name;".to_string(),
            "schema" => "SELECT sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type DESC, name;".to_string(),
            "query" => {
                match args["query"].as_str() {
                    Some(q) => q.to_string(),
                    None => {
                        return Ok(ToolOutput {
                            tool_use_id: input.tool_use_id,
                            content: "The 'query' field is required for action='query'.".to_string(),
                            is_error: true,
                            metadata: None,
                        });
                    }
                }
            }
            _ => {
                return Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content: format!("Unknown action: {}", action),
                    is_error: true,
                    metadata: None,
                });
            }
        };

        // Note: read-only enforcement is guaranteed by SQLITE_OPEN_READ_ONLY in execute_query_sync.
        // String-matching heuristics were removed — they generated false negatives with CTEs.
        let db_path_clone = db_path.clone();
        let sql_clone = sql.clone();

        let result = tokio::task::spawn_blocking(move || {
            SqlQueryTool::execute_query_sync(&db_path_clone, &sql_clone)
        })
        .await;

        match result {
            Err(e) => Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: format!("Internal error: {}", e),
                is_error: true,
                metadata: None,
            }),
            Ok(Err(e)) => Ok(ToolOutput {
                tool_use_id: input.tool_use_id,
                content: e,
                is_error: true,
                metadata: None,
            }),
            Ok(Ok(qr)) => {
                let row_count = qr.rows.len();
                let truncated = qr.truncated;
                let table = Self::format_table(&qr);

                let mut content = format!(
                    "Query: {}\nDatabase: {}\nRows: {}{}\n\n{}",
                    &sql[..sql.len().min(100)],
                    db_path.display(),
                    row_count,
                    if truncated {
                        format!(" (truncated, showing first {})", MAX_ROWS)
                    } else {
                        String::new()
                    },
                    table
                );

                if content.len() > 20_000 {
                    content.truncate(20_000);
                    content.push_str("\n... [output truncated]");
                }

                Ok(ToolOutput {
                    tool_use_id: input.tool_use_id,
                    content,
                    is_error: false,
                    metadata: Some(json!({
                        "rows": row_count,
                        "truncated": truncated,
                        "database": db_path.to_string_lossy()
                    })),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_db(dir: &Path) -> PathBuf {
        let db_path = dir.join("test.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, age INTEGER);
             INSERT INTO users VALUES (1, 'Alice', 30);
             INSERT INTO users VALUES (2, 'Bob', 25);
             INSERT INTO users VALUES (3, 'Carol', 35);",
        )
        .unwrap();
        db_path
    }

    #[test]
    fn execute_query_sync_basic() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(dir.path());
        let result =
            SqlQueryTool::execute_query_sync(&db_path, "SELECT * FROM users ORDER BY id").unwrap();
        assert_eq!(result.columns, vec!["id", "name", "age"]);
        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.rows[0][1], "Alice");
    }

    #[test]
    fn execute_query_sync_empty_result() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(dir.path());
        let result =
            SqlQueryTool::execute_query_sync(&db_path, "SELECT * FROM users WHERE age > 100")
                .unwrap();
        assert!(result.rows.is_empty());
    }

    #[test]
    fn format_table_basic() {
        let qr = QueryResult {
            columns: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec!["1".to_string(), "Alice".to_string()],
                vec!["2".to_string(), "Bob".to_string()],
            ],
            truncated: false,
        };
        let table = SqlQueryTool::format_table(&qr);
        assert!(table.contains("Alice"));
        assert!(table.contains("Bob"));
        assert!(table.contains("+"));
        assert!(table.contains("|"));
    }

    #[test]
    fn format_table_empty() {
        let qr = QueryResult {
            columns: vec!["id".to_string()],
            rows: vec![],
            truncated: false,
        };
        let table = SqlQueryTool::format_table(&qr);
        assert!(table.contains("no rows"));
    }

    #[tokio::test]
    async fn execute_list_dbs() {
        let dir = TempDir::new().unwrap();
        create_test_db(dir.path());
        let tool = SqlQueryTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "action": "list_dbs" }),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("test.db"));
    }

    #[tokio::test]
    async fn execute_tables_action() {
        let dir = TempDir::new().unwrap();
        create_test_db(dir.path());
        let tool = SqlQueryTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({ "database": "test.db", "action": "tables" }),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        assert!(out.content.contains("users"), "content: {}", out.content);
    }

    #[tokio::test]
    async fn execute_select_query() {
        let dir = TempDir::new().unwrap();
        create_test_db(dir.path());
        let tool = SqlQueryTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({
                    "database": "test.db",
                    "query": "SELECT name FROM users WHERE age > 28 ORDER BY name"
                }),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(!out.is_error, "error: {}", out.content);
        assert!(
            out.content.contains("Alice") || out.content.contains("Carol"),
            "content: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn execute_rejects_write_query() {
        let dir = TempDir::new().unwrap();
        create_test_db(dir.path());
        let tool = SqlQueryTool::new();
        let out = tool
            .execute(ToolInput {
                tool_use_id: "t1".into(),
                arguments: json!({
                    "database": "test.db",
                    "query": "DELETE FROM users"
                }),
                working_directory: dir.path().to_str().unwrap().to_string(),
            })
            .await
            .unwrap();
        assert!(out.is_error);
        // "readonly" = SQLite's own error message (SQLITE_OPEN_READ_ONLY enforcement).
        // "rejected" = old heuristic message (kept for test compatibility, no longer emitted).
        // "read-only" = legacy heuristic wording.
        assert!(
            out.content.contains("rejected")
                || out.content.contains("read-only")
                || out.content.contains("readonly"),
            "expected write rejection but got: {:?}",
            out.content
        );
    }

    #[test]
    fn tool_metadata() {
        let t = SqlQueryTool::default();
        assert_eq!(t.name(), "sql_query");
        assert!(!t.description().is_empty());
        assert_eq!(t.permission_level(), PermissionLevel::ReadOnly);
    }
}
