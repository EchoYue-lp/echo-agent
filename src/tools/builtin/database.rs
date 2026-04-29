//! 数据库 SQL 工具
//!
//! 通过 sqlx 提供跨数据库的只读查询能力：
//! - sql_query: 执行只读 SQL 查询
//! - list_tables: 列出数据库中的所有表
//! - describe_table: 查看表结构

use futures::future::BoxFuture;
use serde_json::Value;
use sqlx::any::AnyPoolOptions;
use sqlx::{Column, Row};

use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

// ── SQL 查询（只读） ─────────────────────────────────────────────────────────

pub struct SqlQueryTool;

impl Tool for SqlQueryTool {
    fn name(&self) -> &str {
        "sql_query"
    }

    fn description(&self) -> &str {
        "执行只读 SQL 查询（仅允许 SELECT）。支持 SQLite、MySQL、PostgreSQL。\
         连接 URL 格式: sqlite://path.db, mysql://user:pass@host/db, postgresql://user:pass@host/db"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connection_url": {
                    "type": "string",
                    "description": "数据库连接 URL（sqlite:///path.db | mysql://user:pass@host/db | postgresql://user:pass@host/db）"
                },
                "query": {
                    "type": "string",
                    "description": "要执行的 SQL 查询（仅允许 SELECT / SHOW / DESCRIBE / EXPLAIN / PRAGMA）"
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

            // 安全检查：仅允许只读语句
            let trimmed = query.trim().to_uppercase();
            let allowed = trimmed.starts_with("SELECT")
                || trimmed.starts_with("SHOW")
                || trimmed.starts_with("DESCRIBE")
                || trimmed.starts_with("DESC ")
                || trimmed.starts_with("EXPLAIN")
                || trimmed.starts_with("PRAGMA")
                || trimmed.starts_with("WITH"); // CTE 通常跟着 SELECT

            if !allowed {
                return Ok(ToolResult::error(format!(
                    "仅允许只读查询（SELECT/SHOW/DESCRIBE/EXPLAIN/PRAGMA），收到: {}",
                    query
                )));
            }

            // 额外的危险关键词扫描
            let dangerous = [
                "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "GRANT",
                "REVOKE", "REPLACE",
            ];
            for keyword in &dangerous {
                if trimmed.contains(keyword) {
                    return Ok(ToolResult::error(format!(
                        "查询包含禁止的关键词: {}。仅允许只读查询。",
                        keyword
                    )));
                }
            }

            match execute_readonly_query(conn_url, query).await {
                Ok(data) => Ok(ToolResult::success_json(data)),
                Err(e) => Ok(ToolResult::error(format!("查询失败: {}", e))),
            }
        })
    }
}

// ── 列出表 ───────────────────────────────────────────────────────────────────

pub struct ListTablesTool;

impl Tool for ListTablesTool {
    fn name(&self) -> &str {
        "list_tables"
    }

    fn description(&self) -> &str {
        "列出数据库中的所有表。支持 SQLite、MySQL、PostgreSQL。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connection_url": {
                    "type": "string",
                    "description": "数据库连接 URL"
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

            // 根据数据库类型选择合适的查询
            let query = if conn_url.starts_with("sqlite") {
                "SELECT name AS table_name FROM sqlite_master WHERE type='table' ORDER BY name"
            } else if conn_url.starts_with("mysql") {
                "SELECT TABLE_NAME AS table_name FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME"
            } else {
                // PostgreSQL 及其他
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name"
            };

            match execute_readonly_query(conn_url, query).await {
                Ok(data) => Ok(ToolResult::success_json(data)),
                Err(e) => Ok(ToolResult::error(format!("列出表失败: {}", e))),
            }
        })
    }
}

// ── 描述表结构 ───────────────────────────────────────────────────────────────

pub struct DescribeTableTool;

impl Tool for DescribeTableTool {
    fn name(&self) -> &str {
        "describe_table"
    }

    fn description(&self) -> &str {
        "查看指定表的结构（列名、类型、是否可空）。支持 SQLite、MySQL、PostgreSQL。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connection_url": {
                    "type": "string",
                    "description": "数据库连接 URL"
                },
                "table_name": {
                    "type": "string",
                    "description": "要查看的表名"
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

            // 根据数据库类型选择合适的查询
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
                Err(e) => Ok(ToolResult::error(format!("查询表结构失败: {}", e))),
            }
        })
    }
}

// ── Helper ───────────────────────────────────────────────────────────────────

/// 执行只读查询，返回结构化 JSON
async fn execute_readonly_query(conn_url: &str, query: &str) -> Result<serde_json::Value> {
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(conn_url)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "database".to_string(),
            message: format!("数据库连接失败: {}", e),
        })?;

    let rows =
        sqlx::query(query)
            .fetch_all(&pool)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "database".to_string(),
                message: format!("查询执行失败: {}", e),
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
