//! Database SQL tools
//!
//! Provides cross-database read-only query capabilities via sqlx:
//! - sql_query: execute read-only SQL queries
//! - list_tables: list all tables in the database
//! - describe_table: view table structure

use futures::future::BoxFuture;
use serde_json::Value;
use sqlx::any::AnyPoolOptions;
use sqlx::{Column, Row};

use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};

// ── SQL Query (read-only) ─────────────────────────────────────────────────────────

pub struct SqlQueryTool;

impl Tool for SqlQueryTool {
    fn name(&self) -> &str {
        "sql_query"
    }

    fn description(&self) -> &str {
        "Execute read-only SQL queries (SELECT only). Supports SQLite, MySQL, PostgreSQL. \
         Connection URL format: sqlite://path.db, mysql://user:pass@host/db, postgresql://user:pass@host/db"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connection_url": {
                    "type": "string",
                    "description": "Database connection URL (sqlite:///path.db | mysql://user:pass@host/db | postgresql://user:pass@host/db)"
                },
                "query": {
                    "type": "string",
                    "description": "SQL query to execute (only SELECT / SHOW / DESCRIBE / EXPLAIN / PRAGMA allowed)"
                }
            },
            "required": ["connection_url", "query"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let conn_url = parameters
                .get("connection_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("connection_url".to_string()))?;

            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

            // Safety check: only allow read-only statements
            let trimmed = query.trim().to_uppercase();
            let allowed = trimmed.starts_with("SELECT")
                || trimmed.starts_with("SHOW")
                || trimmed.starts_with("DESCRIBE")
                || trimmed.starts_with("DESC ")
                || trimmed.starts_with("EXPLAIN")
                || trimmed.starts_with("PRAGMA")
                || trimmed.starts_with("WITH"); // CTE usually followed by SELECT

            if !allowed {
                return Ok(ToolResult::error(format!(
                    "Only read-only queries allowed (SELECT/SHOW/DESCRIBE/EXPLAIN/PRAGMA), received: {}",
                    query
                )));
            }

            // Additional dangerous keyword scan
            let dangerous = [
                "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "GRANT",
                "REVOKE", "REPLACE",
            ];
            for keyword in &dangerous {
                if trimmed.contains(keyword) {
                    return Ok(ToolResult::error(format!(
                        "Query contains forbidden keyword: {}. Only read-only queries allowed.",
                        keyword
                    )));
                }
            }

            match execute_readonly_query(conn_url, query).await {
                Ok(data) => Ok(ToolResult::success_json(data)),
                Err(e) => Ok(ToolResult::error(format!("Query failed: {}", e))),
            }
        })
    }
}

// ── List tables ───────────────────────────────────────────────────────────────────

pub struct ListTablesTool;

impl Tool for ListTablesTool {
    fn name(&self) -> &str {
        "list_tables"
    }

    fn description(&self) -> &str {
        "List all tables in the database. Supports SQLite, MySQL, PostgreSQL."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connection_url": {
                    "type": "string",
                    "description": "Database connection URL"
                }
            },
            "required": ["connection_url"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let conn_url = parameters
                .get("connection_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("connection_url".to_string()))?;

            // Choose appropriate query based on database type
            let query = if conn_url.starts_with("sqlite") {
                "SELECT name AS table_name FROM sqlite_master WHERE type='table' ORDER BY name"
            } else if conn_url.starts_with("mysql") {
                "SELECT TABLE_NAME AS table_name FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME"
            } else {
                // PostgreSQL and others
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name"
            };

            match execute_readonly_query(conn_url, query).await {
                Ok(data) => Ok(ToolResult::success_json(data)),
                Err(e) => Ok(ToolResult::error(format!("List tables failed: {}", e))),
            }
        })
    }
}

// ── Describe table structure ───────────────────────────────────────────────────────────────

pub struct DescribeTableTool;

impl Tool for DescribeTableTool {
    fn name(&self) -> &str {
        "describe_table"
    }

    fn description(&self) -> &str {
        "View the structure of a specified table (column names, types, nullable). Supports SQLite, MySQL, PostgreSQL."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connection_url": {
                    "type": "string",
                    "description": "Database connection URL"
                },
                "table_name": {
                    "type": "string",
                    "description": "Table name to view"
                }
            },
            "required": ["connection_url", "table_name"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let conn_url = parameters
                .get("connection_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("connection_url".to_string()))?;

            let table_name = parameters
                .get("table_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("table_name".to_string()))?;

            // Choose appropriate query based on database type
            let query = if conn_url.starts_with("sqlite") {
                format!("PRAGMA table_info('{}')", table_name.replace('\'', "''"))
            } else if conn_url.starts_with("mysql") {
                format!(
                    "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT \
                     FROM information_schema.COLUMNS \
                     WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = '{}' \
                     ORDER BY ORDINAL_POSITION",
                    table_name.replace('\'', "\\'")
                )
            } else {
                // PostgreSQL
                format!(
                    "SELECT column_name, data_type, is_nullable, column_default \
                     FROM information_schema.columns \
                     WHERE table_name = '{}' \
                     ORDER BY ordinal_position",
                    table_name.replace('\'', "''")
                )
            };

            match execute_readonly_query(conn_url, &query).await {
                Ok(data) => Ok(ToolResult::success_json(data)),
                Err(e) => Ok(ToolResult::error(format!("Describe table failed: {}", e))),
            }
        })
    }
}

// ── Helper ───────────────────────────────────────────────────────────────────

/// Execute a read-only query and return structured JSON
async fn execute_readonly_query(conn_url: &str, query: &str) -> Result<serde_json::Value> {
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(conn_url)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "database".to_string(),
            message: format!("Database connection failed: {}", e),
        })?;

    let rows =
        sqlx::query(query)
            .fetch_all(&pool)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "database".to_string(),
                message: format!("Query execution failed: {}", e),
            })?;

    let columns: Vec<String> = if rows.is_empty() {
        vec![]
    } else {
        rows[0]
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect()
    };

    let col_count = columns.len();
    let mut row_values: Vec<Vec<serde_json::Value>> = Vec::with_capacity(rows.len());

    for row in &rows {
        let mut values: Vec<serde_json::Value> = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let val = match row.try_get::<Option<String>, _>(i) {
                Ok(None) => serde_json::Value::Null,
                Ok(Some(s)) => serde_json::Value::String(s),
                Err(_) => match row.try_get::<String, _>(i) {
                    Ok(s) => serde_json::Value::String(s),
                    Err(_) => serde_json::Value::String("?".to_string()),
                },
            };
            values.push(val);
        }
        row_values.push(values);
    }

    Ok(serde_json::json!({
        "columns": columns,
        "rows": row_values,
        "total_rows": row_values.len(),
    }))
}
